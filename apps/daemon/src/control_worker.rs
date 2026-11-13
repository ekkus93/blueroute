//! P7-004 daemon-owned inbound NAP/TLS listener (software-first).
//!
//! An independent worker keeps synchronous TLS I/O off the D-Bus executor.
//! Only durable enrolled pins can pass TLS and Hello. Active sessions poll
//! replay-checked Heartbeat/Goodbye under bounded per-peer read slices.
//! This does not enable production JoinNetwork or prove Bluetooth radio behavior.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use blueroute_core::{NetworkId, NodeCapabilities, NodeId, Sourced};
use blueroute_linux::NetworkMembershipStore;
use blueroute_protocol::control::{ControlMessage, HelloFacts};
use blueroute_protocol::session::{ConnectionDeadline, ConnectionStage, ControlSessionRegistry};
use futures_lite::future;

use crate::control_tls::ControlSessionRead;
use crate::{
    ApprovedControlSetup, DurablePeerTrustOperations, LocalControlTlsIdentity, NetworkOperations,
    ObservedControlListener, RegisteredControlSession, current_network,
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const REBIND_INTERVAL: Duration = Duration::from_secs(2);
const MAX_OWNED_SESSIONS: usize = 32;
const SESSION_READ_SLICE: Duration = Duration::from_millis(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const HEARTBEAT_WRITE_BUDGET: Duration = Duration::from_millis(100);
const GOODBYE_WRITE_BUDGET: Duration = Duration::from_millis(100);
const ACTIVE_CLOCK_BASE_MS: u64 = 3;

/// The peers that currently hold an authenticated control session on this host (P6-007
/// diagnostics). Updated by the worker every tick; empty when no listener is bound.
#[derive(Clone, Default)]
pub struct AuthenticatedControlPeers(Arc<Mutex<Vec<NodeId>>>);

impl AuthenticatedControlPeers {
    /// Sorted and without duplicates. A poisoned lock reads as empty rather than stale.
    pub fn snapshot(&self) -> Vec<NodeId> {
        self.0.lock().map(|peers| peers.clone()).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn set_for_tests(&self, peers: Vec<NodeId>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = peers;
        }
    }

    fn publish(&self, sessions: &[RegisteredControlSession<'_>]) {
        let mut peers: Vec<NodeId> = sessions.iter().map(|session| session.peer()).collect();
        peers.sort_by_key(|peer| *peer.as_bytes());
        peers.dedup();
        if let Ok(mut slot) = self.0.lock() {
            *slot = peers;
        }
    }
}

/// A daemon-lifetime owner for the NAP control worker.
///
/// Drop requests cancellation and wakes the worker's idle park. Call
/// `shutdown` to also wait for the worker to close all owned TLS sessions.
/// An in-flight bounded handshake or backend observation may delay the join.
pub struct DaemonControlWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    peers: AuthenticatedControlPeers,
}

impl DaemonControlWorker {
    /// A live view of the host's authenticated control peers, for diagnostics.
    pub fn authenticated_peers(&self) -> AuthenticatedControlPeers {
        self.peers.clone()
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = &self.handle {
            handle.thread().unpark();
        }
    }

    /// Request orderly teardown and wait for the worker thread to finish.
    pub fn shutdown(mut self) -> thread::Result<()> {
        self.request_stop();
        self.handle.take().expect("worker handle").join()
    }
}

impl Drop for DaemonControlWorker {
    fn drop(&mut self) {
        // Do not block an async daemon executor on thread shutdown in Drop.
        self.request_stop();
    }
}

/// Retain this owner for the daemon lifetime. No active NAP means no
/// listener; a newly hosted NAP is discovered by a fresh guarded bind.
pub fn spawn_daemon_control_worker(
    membership: NetworkMembershipStore,
    operations: Arc<dyn NetworkOperations>,
    trust: Arc<DurablePeerTrustOperations>,
    local: LocalControlTlsIdentity,
    capabilities: NodeCapabilities,
) -> std::io::Result<DaemonControlWorker> {
    spawn_daemon_control_worker_with_peers(
        membership,
        operations,
        trust,
        local,
        capabilities,
        AuthenticatedControlPeers::default(),
    )
}

/// As `spawn_daemon_control_worker`, publishing into a handle created beforehand (so the D-Bus
/// service can hold it before the worker starts).
pub fn spawn_daemon_control_worker_with_peers(
    membership: NetworkMembershipStore,
    operations: Arc<dyn NetworkOperations>,
    trust: Arc<DurablePeerTrustOperations>,
    local: LocalControlTlsIdentity,
    capabilities: NodeCapabilities,
    peers: AuthenticatedControlPeers,
) -> std::io::Result<DaemonControlWorker> {
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_peers = peers.clone();
    let handle = thread::Builder::new()
        .name("blueroute-control-tls".to_owned())
        .spawn(move || {
            run_worker(
                membership,
                operations,
                trust,
                local,
                capabilities,
                worker_stop,
                &worker_peers,
            );
            // Nothing is supervised any more once the worker has exited.
            worker_peers.publish(&[]);
        })?;
    Ok(DaemonControlWorker {
        stop,
        handle: Some(handle),
        peers,
    })
}

fn control_setup(network: NetworkId, capabilities: NodeCapabilities) -> ApprovedControlSetup {
    // This Active clock is advanced by the verified inbound frame reader.
    // Its 30-second idle expiry is never refreshed by an empty socket poll.
    let mut active_deadline = ConnectionDeadline::new(0);
    active_deadline
        .advance(ConnectionStage::TlsHandshake, 1)
        .expect("stage");
    active_deadline
        .advance(ConnectionStage::Hello, 2)
        .expect("stage");
    active_deadline
        .advance(ConnectionStage::Active, 3)
        .expect("stage");
    ApprovedControlSetup {
        network,
        facts: HelloFacts {
            software_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities,
            segment: None,
            direct_neighbors: Vec::new(),
        },
        handshake_deadline: Instant::now() + Duration::from_secs(5),
        active_deadline,
    }
}

// Map each verified generation's admission Instant onto the Active
// ConnectionDeadline's logical clock. A regressed or overflowing clock
// cannot grant extra session lifetime.
fn session_clock_ms(admitted: Instant, now: Instant) -> Option<u64> {
    let elapsed = now.checked_duration_since(admitted)?;
    u64::try_from(elapsed.as_millis())
        .ok()?
        .checked_add(ACTIVE_CLOCK_BASE_MS)
}

/// P6-007: admit a newly authenticated peer only while the hosted star is below the local NAP
/// policy ceiling. A peer that already holds a session never counts twice, and an unknown or zero
/// ceiling refuses everyone (fail closed). Only the ceiling is read from the startup capability
/// snapshot: it is configuration, not an observation, and NAP ownership is observed separately.
fn within_policy_ceiling(
    ceiling: Option<&Sourced<u16>>,
    active: impl IntoIterator<Item = NodeId>,
    candidate: NodeId,
) -> Result<(), &'static str> {
    let ceiling = match ceiling {
        Some(ceiling) if ceiling.value > 0 => usize::from(ceiling.value),
        _ => return Err("the NAP client policy ceiling is unknown"),
    };
    let peers: std::collections::BTreeSet<[u8; 16]> =
        active.into_iter().map(|peer| *peer.as_bytes()).collect();
    if peers.contains(candidate.as_bytes()) || peers.len() < ceiling {
        Ok(())
    } else {
        Err("the NAP client policy ceiling is reached")
    }
}

fn heartbeat_due(last_sent: Instant, now: Instant) -> bool {
    now.checked_duration_since(last_sent)
        .is_some_and(|elapsed| elapsed >= HEARTBEAT_INTERVAL)
}

fn run_worker(
    membership: NetworkMembershipStore,
    operations: Arc<dyn NetworkOperations>,
    trust: Arc<DurablePeerTrustOperations>,
    local: LocalControlTlsIdentity,
    capabilities: NodeCapabilities,
    stop: Arc<AtomicBool>,
    peers: &AuthenticatedControlPeers,
) {
    let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
    let mut listener: Option<ObservedControlListener> = None;
    let mut sessions: Vec<RegisteredControlSession<'_>> = Vec::new();
    let mut registered_at: BTreeMap<[u8; 16], Instant> = BTreeMap::new();
    let mut heartbeat_sent_at: BTreeMap<[u8; 16], Instant> = BTreeMap::new();
    while !stop.load(Ordering::Acquire) {
        if listener.is_none() {
            sessions.clear();
            registered_at.clear();
            heartbeat_sent_at.clear();
            peers.publish(&sessions);
            // A backend may still report a stale owned NAP after a local
            // leave or corrupted membership state. Never bind a TCP ingress
            // listener until durable local-host membership is valid.
            match membership.load().and_then(|saved| current_network(&saved)) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    thread::park_timeout(REBIND_INTERVAL);
                    continue;
                }
                Err(error) => {
                    eprintln!(
                        "BlueRoute NAP membership preflight denied: {:?}",
                        error.kind()
                    );
                    thread::park_timeout(REBIND_INTERVAL);
                    continue;
                }
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            let bound = future::block_on(operations.bind_guarded_control_listener());
            // Shutdown can race an awaited backend bind. Its newly owned TCP
            // listener must be dropped without further membership observations
            // or session admission once cancellation was requested.
            if stop.load(Ordering::Acquire) {
                break;
            }
            match bound {
                Ok(Some(bound)) => {
                    // A durable leave or network switch can race the backend
                    // bind. Drop the newly bound TCP socket before handing it
                    // to the supervisor unless its local network still agrees
                    // with a *fresh* durable membership read. The subsequent
                    // supervision tick also checks the observed NAP endpoint.
                    match membership.load().and_then(|saved| current_network(&saved)) {
                        Ok(Some(network)) if network == bound.network() => listener = Some(bound),
                        Ok(_) => {
                            eprintln!("BlueRoute NAP listener bind raced durable network change")
                        }
                        Err(error) => eprintln!(
                            "BlueRoute NAP listener post-bind membership denied: {:?}",
                            error.kind()
                        ),
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("BlueRoute NAP listener bind denied: {:?}", error.kind()),
            }
            if listener.is_none() {
                thread::park_timeout(REBIND_INTERVAL);
                continue;
            }
        }
        let owner = listener.as_mut().expect("checked bound listener");
        if let Err(error) =
            future::block_on(owner.supervise_registered_from_durable_network_operations(
                &mut sessions,
                &membership,
                operations.as_ref(),
            ))
        {
            eprintln!("BlueRoute NAP listener retired: {:?}", error.kind());
            sessions.clear();
            registered_at.clear();
            heartbeat_sent_at.clear();
            listener = None;
            thread::park_timeout(REBIND_INTERVAL);
            continue;
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
        // Revocation may have removed a session during the fresh durability
        // recheck. Release its matching provisional lease as well.
        registered_at.retain(|session_id, _| {
            sessions
                .iter()
                .any(|session| session.session_id() == session_id)
        });
        heartbeat_sent_at.retain(|session_id, _| {
            sessions
                .iter()
                .any(|session| session.session_id() == session_id)
        });
        // Validate durable NAP ownership before any authenticated peer I/O.
        // A retired listener must close its sessions before the worker can
        // consume or emit a Heartbeat on an obsolete network interface.
        // Read each authenticated generation for at most one short slice,
        // allowing other peers and fresh NAP observations to make progress.
        // The ingress layer enforces the full idle deadline, replay and pins.
        sessions.retain_mut(|session| {
            if stop.load(Ordering::Acquire) {
                return false;
            }
            let id = *session.session_id();
            let Some(admitted) = registered_at.get(&id).copied() else {
                return false;
            };
            let Some(now_ms) = session_clock_ms(admitted, Instant::now()) else {
                return false;
            };
            match session.receive_next_with_budget_at(now_ms, SESSION_READ_SLICE) {
                Ok(ControlSessionRead::Frames(frames)) => {
                    if frames
                        .iter()
                        .any(|frame| matches!(frame.message, ControlMessage::Goodbye))
                    {
                        // A peer-requested shutdown is terminal for this
                        // generation. Reply best-effort, then close and retire.
                        if let Some(deadline) = Instant::now().checked_add(GOODBYE_WRITE_BUDGET) {
                            let _ = session.send_goodbye_until(deadline);
                        }
                        return false;
                    }
                }
                Ok(ControlSessionRead::Idle) => {}
                Ok(ControlSessionRead::Closed) | Err(_) => return false,
            }
            let now = Instant::now();
            let Some(last_sent) = heartbeat_sent_at.get(&id).copied() else {
                return false;
            };
            if heartbeat_due(last_sent, now) {
                let Some(deadline) = now.checked_add(HEARTBEAT_WRITE_BUDGET) else {
                    return false;
                };
                if session.send_heartbeat_until(deadline).is_err() {
                    return false;
                }
                heartbeat_sent_at.insert(id, Instant::now());
            }
            true
        });
        registered_at.retain(|session_id, _| {
            sessions
                .iter()
                .any(|session| session.session_id() == session_id)
        });
        heartbeat_sent_at.retain(|session_id, _| {
            sessions
                .iter()
                .any(|session| session.session_id() == session_id)
        });

        if stop.load(Ordering::Acquire) {
            break;
        }
        if sessions.len() < MAX_OWNED_SESSIONS {
            // Never take a peer-provided network as the expected identity.
            // Listener admission rechecks this durable value on each gate.
            let expected = membership.load().and_then(|saved| current_network(&saved));
            if let Ok(Some(network)) = expected {
                match future::block_on(owner.poll_registered_from_durable_network_operations(
                    &membership,
                    operations.as_ref(),
                    &local,
                    trust.as_ref(),
                    control_setup(network, capabilities.clone()),
                    Arc::clone(&registry),
                )) {
                    Ok(Some(mut registered)) => {
                        if let Err(reason) = within_policy_ceiling(
                            capabilities.connection_policy_ceiling.as_ref(),
                            sessions.iter().map(RegisteredControlSession::peer),
                            registered.peer(),
                        ) {
                            // Tell the peer, then drop the session: that closes the socket and
                            // retires its registry generation. Its PANU link is not touched.
                            eprintln!("BlueRoute refused an authenticated peer: {reason}");
                            if let Some(deadline) = Instant::now().checked_add(GOODBYE_WRITE_BUDGET)
                            {
                                let _ = registered.send_goodbye_until(deadline);
                            }
                            drop(registered);
                            peers.publish(&sessions);
                            thread::park_timeout(POLL_INTERVAL);
                            continue;
                        }
                        let admitted = Instant::now();
                        registered_at.insert(*registered.session_id(), admitted);
                        heartbeat_sent_at.insert(*registered.session_id(), admitted);
                        sessions.push(registered);
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("BlueRoute inbound TLS denied: {:?}", error.kind()),
                }
            } else {
                sessions.clear();
                registered_at.clear();
                heartbeat_sent_at.clear();
                listener = None;
            }
        }
        peers.publish(&sessions);
        thread::park_timeout(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {

    /// Test hosts carry an explicit policy ceiling; an unknown ceiling refuses every peer.
    fn worker_capabilities() -> NodeCapabilities {
        NodeCapabilities {
            connection_policy_ceiling: Some(blueroute_core::Sourced::new(
                32,
                blueroute_core::CapabilitySource::Configured,
            )),
            ..NodeCapabilities::default()
        }
    }
    use super::*;

    fn save_host_membership(path: &std::path::Path) {
        use blueroute_core::{DisplayName, MembershipRegistry, MembershipState, NetworkMembership};
        let mut membership = NetworkMembership::new(
            NetworkId::from_bytes([7; 16]),
            DisplayName::new("Test host").unwrap(),
        );
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        NetworkMembershipStore::new(path.to_path_buf())
            .save(&registry)
            .unwrap();
    }

    #[derive(Default)]
    struct NoNapOperations {
        bind_attempts: std::sync::atomic::AtomicUsize,
        fail_bind: AtomicBool,
    }

    impl NetworkOperations for NoNapOperations {
        fn bind_guarded_control_listener(
            &self,
        ) -> crate::NetworkOperationFuture<'_, Option<ObservedControlListener>> {
            Box::pin(async move {
                self.bind_attempts.fetch_add(1, Ordering::SeqCst);
                if self.fail_bind.load(Ordering::SeqCst) {
                    Err(blueroute_core::CoreError::new(
                        blueroute_core::ErrorKind::InvalidState,
                        "simulated transient NAP bind failure",
                    ))
                } else {
                    Ok(None)
                }
            })
        }

        fn create_network(
            &self,
            _: blueroute_core::DisplayName,
        ) -> crate::NetworkOperationFuture<'_, NetworkId> {
            Box::pin(async { unreachable!("idle worker must not create networks") })
        }

        fn leave_network(&self) -> crate::NetworkOperationFuture<'_, ()> {
            Box::pin(async { unreachable!("idle worker must not leave networks") })
        }

        fn list_networks(
            &self,
        ) -> crate::NetworkOperationFuture<'_, Vec<blueroute_protocol::NetworkSummary>> {
            Box::pin(async { unreachable!("idle worker must not list networks") })
        }

        fn start_discovery(&self) -> crate::NetworkOperationFuture<'_, ()> {
            Box::pin(async { unreachable!("idle worker must not discover peers") })
        }

        fn stop_discovery(&self) -> crate::NetworkOperationFuture<'_, ()> {
            Box::pin(async { unreachable!("idle worker must not stop discovery") })
        }
    }

    /// A real loopback socket models an already-owned NAP listener. It is
    /// never evidence of Bluetooth transport or peer enrollment.
    #[derive(Default)]
    struct HostedNapOperations {
        endpoint: Mutex<Option<std::net::SocketAddrV4>>,
        ownership_lost: AtomicBool,
        binds: std::sync::atomic::AtomicUsize,
        observed_endpoints: std::sync::atomic::AtomicUsize,
        bind_gate: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    }

    impl NetworkOperations for HostedNapOperations {
        fn bind_guarded_control_listener(
            &self,
        ) -> crate::NetworkOperationFuture<'_, Option<ObservedControlListener>> {
            Box::pin(async move {
                if self.ownership_lost.load(Ordering::Acquire) {
                    return Ok(None);
                }
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let std::net::SocketAddr::V4(endpoint) = listener.local_addr().unwrap() else {
                    unreachable!("test binds IPv4 only");
                };
                *self.endpoint.lock().unwrap() = Some(endpoint);
                self.binds.fetch_add(1, Ordering::SeqCst);
                if let Some((entered, release)) = self.bind_gate.lock().unwrap().take() {
                    entered.send(()).unwrap();
                    release.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                Ok(Some(ObservedControlListener::new(
                    NetworkId::from_bytes([7; 16]),
                    endpoint,
                    listener,
                )?))
            })
        }

        fn observed_owned_control_endpoint(
            &self,
        ) -> crate::NetworkOperationFuture<'_, Option<std::net::SocketAddrV4>> {
            Box::pin(async move {
                self.observed_endpoints.fetch_add(1, Ordering::SeqCst);
                if self.ownership_lost.load(Ordering::Acquire) {
                    Ok(None)
                } else {
                    Ok(*self.endpoint.lock().unwrap())
                }
            })
        }

        fn create_network(
            &self,
            _: blueroute_core::DisplayName,
        ) -> crate::NetworkOperationFuture<'_, NetworkId> {
            Box::pin(async { unreachable!("worker must not create networks") })
        }

        fn leave_network(&self) -> crate::NetworkOperationFuture<'_, ()> {
            Box::pin(async { unreachable!("worker must not leave networks") })
        }

        fn list_networks(
            &self,
        ) -> crate::NetworkOperationFuture<'_, Vec<blueroute_protocol::NetworkSummary>> {
            Box::pin(async { unreachable!("worker must not list networks") })
        }

        fn start_discovery(&self) -> crate::NetworkOperationFuture<'_, ()> {
            Box::pin(async { unreachable!("worker must not discover peers") })
        }

        fn stop_discovery(&self) -> crate::NetworkOperationFuture<'_, ()> {
            Box::pin(async { unreachable!("worker must not stop discovery") })
        }
    }

    #[test]
    fn worker_does_not_bind_stale_nap_without_durable_host_membership() {
        use blueroute_core::NodeId;

        let path = std::env::temp_dir().join(format!(
            "blueroute-control-unowned-prebind-{}",
            std::process::id()
        ));
        let operations = Arc::new(NoNapOperations::default());
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path.clone()),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(path.clone()),
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();

        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            operations.bind_attempts.load(Ordering::SeqCst),
            0,
            "a stale backend NAP cannot open control ingress without durable membership"
        );

        // A valid hosted membership becomes eligible on the next bounded
        // rebind tick; the initial denial must not permanently poison startup.
        save_host_membership(&path);
        let deadline = Instant::now() + Duration::from_secs(4);
        while operations.bind_attempts.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "fresh durable membership should permit a guarded bind attempt"
            );
            thread::sleep(Duration::from_millis(10));
        }
        worker.shutdown().unwrap();
    }

    #[test]
    fn shutdown_releases_actual_guarded_listener_socket() {
        use blueroute_core::{
            DisplayName, MembershipRegistry, MembershipState, NetworkMembership, NodeId,
        };

        let path =
            std::env::temp_dir().join(format!("blueroute-control-hosted-{}", std::process::id()));
        let store = NetworkMembershipStore::new(path.clone());
        let network = NetworkId::from_bytes([7; 16]);
        let mut membership =
            NetworkMembership::new(network, DisplayName::new("Test host").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        store.save(&registry).unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            store,
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();
        let wait_until = Instant::now() + Duration::from_secs(2);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < wait_until,
                "worker did not bind a test listener"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert!(
            std::net::TcpListener::bind(endpoint).is_err(),
            "worker must retain its active guarded listener"
        );
        let started = Instant::now();
        worker.shutdown().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            std::net::TcpListener::bind(endpoint).is_ok(),
            "shutdown must release the real listening socket"
        );
    }

    #[test]
    fn running_worker_closes_authenticated_session_on_durable_revocation() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;

        let root = std::env::temp_dir().join(format!(
            "blueroute-control-worker-auth-revoked-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let client_path = root.join("client-membership");
        save_host_membership(&host_path);
        save_host_membership(&client_path);
        let host_store = NetworkMembershipStore::new(host_path.clone());
        let client_store = NetworkMembershipStore::new(client_path);
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let host_pin = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint()).unwrap();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let client_trust = DurablePeerTrustOperations::with_peer_pin_store(
            client_store,
            PeerPinStore::new(root.join("client-pins")),
        );

        // These local fixtures emulate completed enrollment. Production can
        // only persist a pin after independent confirmation and genuine TLS.
        future::block_on(host_trust.trust_peer(client.node())).unwrap();
        host_trust
            .enroll_verified_key(
                network,
                client.node(),
                client.fingerprint(),
                client.fingerprint(),
            )
            .unwrap();
        future::block_on(client_trust.trust_peer(host.node())).unwrap();
        client_trust
            .enroll_verified_key(network, host.node(), host.fingerprint(), host.fingerprint())
            .unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            host_store,
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < ready_before,
                "guarded test NAP did not bind"
            );
            thread::sleep(Duration::from_millis(5));
        };
        let socket = std::net::TcpStream::connect(endpoint).unwrap();
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut session = crate::connect_registered_control_session(
            socket,
            &client,
            host_pin,
            &client_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&registry),
        )
        .expect("real TLS and exporter-bound Hello must authenticate the running worker");
        assert_eq!(session.peer(), NodeId::from_bytes([8; 16]));
        session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();

        // Revoke after genuine TLS admission while the client still owns its
        // socket. The worker must observe the durable approval change, close
        // that transport, and preserve the unrelated host NAP listener.
        future::block_on(host_trust.forget_peer(client.node())).unwrap();
        let read_start = Instant::now();
        let read_until = read_start + Duration::from_secs(4);
        loop {
            let now_ms = 3 + u64::try_from(read_start.elapsed().as_millis()).unwrap();
            match session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                    assert!(
                        Instant::now() < read_until,
                        "the worker kept an authenticated socket after durable revocation"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => break,
            }
        }
        assert!(session.is_terminal());
        assert!(
            host_trust
                .approved_control_pins(network)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            Some(network)
        );
        assert!(
            std::net::TcpListener::bind(endpoint).is_err(),
            "revoking one peer must not drop the host listener"
        );
        worker.shutdown().unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_worker_revokes_one_of_two_authenticated_peers_without_dropping_other() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;

        let root = std::env::temp_dir().join(format!(
            "blueroute-control-worker-two-peers-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let first_path = root.join("first-membership");
        let second_path = root.join("second-membership");
        for path in [&host_path, &first_path, &second_path] {
            save_host_membership(path);
        }
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let first = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let second = LocalControlTlsIdentity::generate(NodeId::from_bytes([10; 16])).unwrap();
        let host_node = host.node();
        let host_fingerprint = host.fingerprint();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let first_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(first_path),
            PeerPinStore::new(root.join("first-pins")),
        );
        let second_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(second_path),
            PeerPinStore::new(root.join("second-pins")),
        );
        for peer in [&first, &second] {
            future::block_on(host_trust.trust_peer(peer.node())).unwrap();
            host_trust
                .enroll_verified_key(network, peer.node(), peer.fingerprint(), peer.fingerprint())
                .unwrap();
        }
        for trust in [&first_trust, &second_trust] {
            future::block_on(trust.trust_peer(host_node)).unwrap();
            trust
                .enroll_verified_key(network, host_node, host_fingerprint, host_fingerprint)
                .unwrap();
        }

        // Test pins emulate prior independent enrollment; only the TLS
        // handshake and the running daemon worker are exercised here.
        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(host_path.clone()),
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < ready_before,
                "guarded test NAP did not bind"
            );
            thread::sleep(Duration::from_millis(5));
        };
        // Each client has its own registry: both see the same host NodeId,
        // while the host worker owns the shared inbound peer registry.
        let first_registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let second_registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut first_session = crate::connect_registered_control_session(
            std::net::TcpStream::connect(endpoint).unwrap(),
            &first,
            PinnedControlTlsPeer::new(network, host_node, host_fingerprint).unwrap(),
            &first_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&first_registry),
        )
        .expect("first real mutual-TLS channel must authenticate");
        let mut second_session = crate::connect_registered_control_session(
            std::net::TcpStream::connect(endpoint).unwrap(),
            &second,
            PinnedControlTlsPeer::new(network, host_node, host_fingerprint).unwrap(),
            &second_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&second_registry),
        )
        .expect("second real mutual-TLS channel must authenticate");
        first_session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        second_session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        // Diagnostics list both authenticated peers, sorted, once the worker has admitted them.
        let peers = worker.authenticated_peers();
        let mut both = vec![first.node(), second.node()];
        both.sort_by_key(|node| *node.as_bytes());
        let listed_by = Instant::now() + Duration::from_secs(3);
        while peers.snapshot() != both {
            assert!(
                Instant::now() < listed_by,
                "peers not published: {:?}",
                peers.snapshot()
            );
            thread::sleep(Duration::from_millis(5));
        }

        future::block_on(host_trust.forget_peer(first.node())).unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(4);
        loop {
            let now_ms =
                ACTIVE_CLOCK_BASE_MS + u64::try_from(started.elapsed().as_millis()).unwrap();
            match first_session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                    assert!(
                        Instant::now() < deadline,
                        "revoked peer retained TLS socket"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => break,
            }
        }
        assert!(first_session.is_terminal());
        assert_eq!(host_trust.approved_control_pins(network).unwrap().len(), 1);
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            Some(network)
        );
        assert!(std::net::TcpListener::bind(endpoint).is_err());
        // Revoking the first peer must not retire the unrelated registered
        // generation or prevent it from exchanging another control frame.
        assert!(!second_session.is_terminal());
        // The revoked peer drops out of the published list; the other stays.
        let narrowed_by = Instant::now() + Duration::from_secs(3);
        while peers.snapshot() != vec![second.node()] {
            assert!(Instant::now() < narrowed_by, "revoked peer still listed");
            thread::sleep(Duration::from_millis(5));
        }
        second_session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        let now_ms = ACTIVE_CLOCK_BASE_MS + u64::try_from(started.elapsed().as_millis()).unwrap();
        assert!(matches!(
            second_session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)),
            Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_))
        ));

        worker.shutdown().unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(4);
        loop {
            let now_ms =
                ACTIVE_CLOCK_BASE_MS + u64::try_from(started.elapsed().as_millis()).unwrap();
            match second_session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                    assert!(
                        Instant::now() < deadline,
                        "shutdown retained second TLS socket"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => break,
            }
        }
        assert!(second_session.is_terminal());
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        // Nothing is supervised after shutdown.
        assert!(peers.snapshot().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn assert_authenticated_malformed_peer_isolation(
        case: &str,
        inject: impl FnOnce(&mut crate::control_tls::ControlTlsStream),
    ) {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;

        let root = std::env::temp_dir().join(format!(
            "blueroute-control-worker-malformed-two-peers-{case}-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let first_path = root.join("first-membership");
        let second_path = root.join("second-membership");
        for path in [&host_path, &first_path, &second_path] {
            save_host_membership(path);
        }
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let first = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let second = LocalControlTlsIdentity::generate(NodeId::from_bytes([10; 16])).unwrap();
        let host_node = host.node();
        let host_fingerprint = host.fingerprint();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let first_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(first_path),
            PeerPinStore::new(root.join("first-pins")),
        );
        let second_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(second_path),
            PeerPinStore::new(root.join("second-pins")),
        );
        for peer in [&first, &second] {
            future::block_on(host_trust.trust_peer(peer.node())).unwrap();
            host_trust
                .enroll_verified_key(network, peer.node(), peer.fingerprint(), peer.fingerprint())
                .unwrap();
        }
        for trust in [&first_trust, &second_trust] {
            future::block_on(trust.trust_peer(host_node)).unwrap();
            trust
                .enroll_verified_key(network, host_node, host_fingerprint, host_fingerprint)
                .unwrap();
        }

        // Test pins emulate prior independent enrollment; only the TLS
        // handshake and the running daemon worker are exercised here.
        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(host_path.clone()),
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < ready_before,
                "guarded test NAP did not bind"
            );
            thread::sleep(Duration::from_millis(5));
        };
        // Each client has its own registry: both see the same host NodeId,
        // while the host worker owns the shared inbound peer registry.
        let first_registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let second_registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        // The first client completes genuine mutual TLS and exporter-bound
        // Hello, but deliberately bypasses the post-Hello writer to inject
        // invalid encrypted control plaintext.
        let (mut hostile, certificate) = crate::connect_pinned_control_tls(
            std::net::TcpStream::connect(endpoint).unwrap(),
            &first,
            network,
            PinnedControlTlsPeer::new(network, host_node, host_fingerprint).unwrap(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        hostile
            .exchange_authenticated_hello(
                network,
                &first,
                &certificate,
                control_setup(network, NodeCapabilities::default()).facts,
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        let mut second_session = crate::connect_registered_control_session(
            std::net::TcpStream::connect(endpoint).unwrap(),
            &second,
            PinnedControlTlsPeer::new(network, host_node, host_fingerprint).unwrap(),
            &second_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&second_registry),
        )
        .expect("unrelated peer must authenticate before malformed traffic");
        second_session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();

        // Keep the hostile TLS socket open: reconnect of the *same* NodeId
        // proves that the worker poisoned and retired only that generation.
        inject(&mut hostile);
        let retry_until = Instant::now() + Duration::from_secs(4);
        let mut recovered = loop {
            let socket = std::net::TcpStream::connect(endpoint).unwrap();
            match crate::connect_registered_control_session(
                socket,
                &first,
                PinnedControlTlsPeer::new(network, host_node, host_fingerprint).unwrap(),
                &first_trust,
                control_setup(network, NodeCapabilities::default()),
                Arc::clone(&first_registry),
            ) {
                Ok(session) => break session,
                Err(error) => {
                    assert!(
                        Instant::now() < retry_until,
                        "malformed authenticated generation was not retired: {error:?}"
                    );
                    thread::sleep(Duration::from_millis(50));
                }
            }
        };
        recovered
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        let started = Instant::now();
        assert_eq!(host_trust.approved_control_pins(network).unwrap().len(), 2);
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            Some(network)
        );
        assert!(std::net::TcpListener::bind(endpoint).is_err());
        // Malformed traffic from the first peer must not retire the unrelated
        // registered generation or prevent another control exchange.
        assert!(!second_session.is_terminal());
        second_session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        let now_ms = ACTIVE_CLOCK_BASE_MS + u64::try_from(started.elapsed().as_millis()).unwrap();
        assert!(matches!(
            second_session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)),
            Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_))
        ));

        worker.shutdown().unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(4);
        loop {
            let now_ms =
                ACTIVE_CLOCK_BASE_MS + u64::try_from(started.elapsed().as_millis()).unwrap();
            match second_session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                    assert!(
                        Instant::now() < deadline,
                        "shutdown retained second TLS socket"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => break,
            }
        }
        assert!(second_session.is_terminal());
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        drop(recovered);
        drop(hostile);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_worker_rejects_one_authenticated_malformed_peer_without_dropping_other() {
        use std::io::Write;

        assert_authenticated_malformed_peer_isolation("oversized", |hostile| {
            hostile.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
            hostile.flush().unwrap();
        });
    }

    #[test]
    fn running_worker_rejects_fragmented_malformed_peer_without_dropping_other() {
        use std::io::Write;

        assert_authenticated_malformed_peer_isolation("fragmented", |hostile| {
            // Cross at least one worker poll with an incomplete length prefix.
            // The worker may reject the partial frame immediately or wait
            // for the remaining bytes; either path must retire this peer only.
            hostile.write_all(&[0xff, 0xff]).unwrap();
            hostile.flush().unwrap();
            thread::sleep(Duration::from_millis(400));
            let _ = hostile.write_all(&[0xff, 0xff]);
            let _ = hostile.flush();
        });
    }

    #[test]
    fn running_worker_rejects_split_invalid_json_without_dropping_other_peer() {
        use std::io::Write;

        assert_authenticated_malformed_peer_isolation("split-invalid-json", |hostile| {
            // The length prefix is valid, but the JSON body is deliberately
            // malformed. Delay its delivery across worker polls to exercise
            // the buffered authenticated-frame path, not just header checks.
            let body = b"{invalid-json";
            hostile
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            hostile.flush().unwrap();
            thread::sleep(Duration::from_millis(400));
            let _ = hostile.write_all(body);
            let _ = hostile.flush();
        });
    }

    #[test]
    fn running_worker_closes_authenticated_session_on_nap_ownership_loss() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;

        let root = std::env::temp_dir().join(format!(
            "blueroute-control-worker-auth-nap-loss-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let client_path = root.join("client-membership");
        save_host_membership(&host_path);
        save_host_membership(&client_path);
        let host_store = NetworkMembershipStore::new(host_path.clone());
        let client_store = NetworkMembershipStore::new(client_path);
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let host_pin = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint()).unwrap();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let client_trust = DurablePeerTrustOperations::with_peer_pin_store(
            client_store,
            PeerPinStore::new(root.join("client-pins")),
        );

        // These local fixtures emulate completed enrollment. Production can
        // only persist a pin after independent confirmation and genuine TLS.
        future::block_on(host_trust.trust_peer(client.node())).unwrap();
        host_trust
            .enroll_verified_key(
                network,
                client.node(),
                client.fingerprint(),
                client.fingerprint(),
            )
            .unwrap();
        future::block_on(client_trust.trust_peer(host.node())).unwrap();
        client_trust
            .enroll_verified_key(network, host.node(), host.fingerprint(), host.fingerprint())
            .unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            host_store,
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < ready_before,
                "guarded test NAP did not bind"
            );
            thread::sleep(Duration::from_millis(5));
        };
        let socket = std::net::TcpStream::connect(endpoint).unwrap();
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut session = crate::connect_registered_control_session(
            socket,
            &client,
            host_pin,
            &client_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&registry),
        )
        .expect("real TLS and exporter-bound Hello must authenticate the running worker");
        assert_eq!(session.peer(), NodeId::from_bytes([8; 16]));
        session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();

        // Simulate a lost owned NAP after genuine TLS admission. The worker
        // must close the live authenticated transport and its listener,
        // without rewriting still-valid durable membership or peer pins.
        operations.ownership_lost.store(true, Ordering::Release);
        let read_start = Instant::now();
        let read_until = read_start + Duration::from_secs(4);
        loop {
            let now_ms = 3 + u64::try_from(read_start.elapsed().as_millis()).unwrap();
            match session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                    assert!(
                        Instant::now() < read_until,
                        "the worker kept an authenticated socket after NAP ownership loss"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => break,
            }
        }
        assert!(session.is_terminal());
        assert_eq!(host_trust.approved_control_pins(network).unwrap().len(), 1);
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            Some(network)
        );
        assert!(
            std::net::TcpListener::bind(endpoint).is_ok(),
            "lost NAP ownership must release the host listener"
        );
        worker.shutdown().unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_worker_closes_authenticated_session_on_durable_leave() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;

        let root = std::env::temp_dir().join(format!(
            "blueroute-control-worker-auth-leave-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let client_path = root.join("client-membership");
        save_host_membership(&host_path);
        save_host_membership(&client_path);
        let host_store = NetworkMembershipStore::new(host_path.clone());
        let client_store = NetworkMembershipStore::new(client_path);
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let host_pin = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint()).unwrap();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let client_trust = DurablePeerTrustOperations::with_peer_pin_store(
            client_store,
            PeerPinStore::new(root.join("client-pins")),
        );

        // These local fixtures emulate completed enrollment. Production can
        // only persist a pin after independent confirmation and genuine TLS.
        future::block_on(host_trust.trust_peer(client.node())).unwrap();
        host_trust
            .enroll_verified_key(
                network,
                client.node(),
                client.fingerprint(),
                client.fingerprint(),
            )
            .unwrap();
        future::block_on(client_trust.trust_peer(host.node())).unwrap();
        client_trust
            .enroll_verified_key(network, host.node(), host.fingerprint(), host.fingerprint())
            .unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            host_store,
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < ready_before,
                "guarded test NAP did not bind"
            );
            thread::sleep(Duration::from_millis(5));
        };
        let socket = std::net::TcpStream::connect(endpoint).unwrap();
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut session = crate::connect_registered_control_session(
            socket,
            &client,
            host_pin,
            &client_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&registry),
        )
        .expect("real TLS and exporter-bound Hello must authenticate the running worker");
        assert_eq!(session.peer(), NodeId::from_bytes([8; 16]));
        session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();

        // Remove durable host membership while the fake NAP backend still
        // reports its stale endpoint. This must retire both the active TLS
        // channel and the listening socket, without accepting stale ingress.
        NetworkMembershipStore::new(host_path.clone())
            .save(&blueroute_core::MembershipRegistry::default())
            .unwrap();
        let read_start = Instant::now();
        let read_until = read_start + Duration::from_secs(4);
        loop {
            let now_ms = 3 + u64::try_from(read_start.elapsed().as_millis()).unwrap();
            match session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                    assert!(
                        Instant::now() < read_until,
                        "durable leave kept an authenticated socket alive"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => break,
            }
        }
        assert!(session.is_terminal());
        assert!(host_trust.approved_control_pins(network).is_err());
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            None
        );
        assert!(
            std::net::TcpListener::bind(endpoint).is_ok(),
            "durable leave must release the host listener"
        );
        let binds = operations.binds.load(Ordering::SeqCst);
        thread::sleep(REBIND_INTERVAL + Duration::from_millis(300));
        assert_eq!(
            operations.binds.load(Ordering::SeqCst),
            binds,
            "stale backend NAP must not rebind without durable membership"
        );
        worker.shutdown().unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_worker_goodbye_retires_generation_and_allows_reconnect() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;

        let root = std::env::temp_dir().join(format!(
            "blueroute-worker-auth-goodbye-reconnect-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let client_path = root.join("client-membership");
        save_host_membership(&host_path);
        save_host_membership(&client_path);
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let host_pin = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint()).unwrap();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let client_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(client_path),
            PeerPinStore::new(root.join("client-pins")),
        );
        // Synthetic fixture pins stand in for independently confirmed enrollment;
        // the test still proves actual mutual TLS and exporter-bound Hello.
        future::block_on(host_trust.trust_peer(client.node())).unwrap();
        host_trust
            .enroll_verified_key(
                network,
                client.node(),
                client.fingerprint(),
                client.fingerprint(),
            )
            .unwrap();
        future::block_on(client_trust.trust_peer(host.node())).unwrap();
        client_trust
            .enroll_verified_key(network, host.node(), host.fingerprint(), host.fingerprint())
            .unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(host_path.clone()),
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(Instant::now() < ready_before, "guarded NAP did not bind");
            thread::sleep(Duration::from_millis(5));
        };

        // A genuinely authenticated peer ends its generation gracefully. The
        // worker must consume Goodbye, respond, and release its registry slot
        // without retiring the hosted NAP or modifying durable approval.
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let socket = std::net::TcpStream::connect(endpoint).unwrap();
        let mut first = crate::connect_registered_control_session(
            socket,
            &client,
            host_pin,
            &client_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&registry),
        )
        .expect("first pinned TLS/Hello session");
        first
            .send_goodbye_until(Instant::now() + Duration::from_secs(2))
            .unwrap();

        let read_start = Instant::now();
        let reply_until = read_start + Duration::from_secs(4);
        loop {
            let now_ms =
                ACTIVE_CLOCK_BASE_MS + u64::try_from(read_start.elapsed().as_millis()).unwrap();
            match first.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Frames(frames))
                    if frames
                        .iter()
                        .any(|frame| matches!(frame.message, ControlMessage::Goodbye)) =>
                {
                    break;
                }
                Ok(ControlSessionRead::Frames(_) | ControlSessionRead::Idle) => {
                    assert!(
                        Instant::now() < reply_until,
                        "worker never acknowledged authenticated Goodbye"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => {
                    // The worker's Goodbye response is best-effort: the
                    // client's TLS close-notify may race its reply. A
                    // terminal close is acceptable provided the server
                    // retires this generation and admits a fresh one below.
                    break;
                }
            }
        }
        // Releasing the client-side owner alone cannot release the *server*
        // registry slot. The replacement must be accepted by the live worker.
        drop(first);

        let retry_until = Instant::now() + Duration::from_secs(4);
        let mut recovered = loop {
            let socket = std::net::TcpStream::connect(endpoint).unwrap();
            match crate::connect_registered_control_session(
                socket,
                &client,
                host_pin,
                &client_trust,
                control_setup(network, NodeCapabilities::default()),
                Arc::clone(&registry),
            ) {
                Ok(session) => break session,
                Err(error) => {
                    assert!(
                        Instant::now() < retry_until,
                        "running worker did not retire the Goodbye generation: {error:?}"
                    );
                    thread::sleep(Duration::from_millis(50));
                }
            }
        };
        assert_eq!(recovered.peer(), NodeId::from_bytes([8; 16]));
        recovered
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_err());
        assert_eq!(host_trust.approved_control_pins(network).unwrap().len(), 1);
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            Some(network)
        );

        worker.shutdown().unwrap();
        drop(recovered);
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_worker_rejects_authenticated_oversized_frame_and_recovers_same_peer() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "blueroute-worker-auth-hostile-frame-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let client_path = root.join("client-membership");
        save_host_membership(&host_path);
        save_host_membership(&client_path);
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let host_pin = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint()).unwrap();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let client_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(client_path),
            PeerPinStore::new(root.join("client-pins")),
        );
        // Synthetic fixture pins stand in for independently confirmed enrollment;
        // the test still proves actual mutual TLS and exporter-bound Hello.
        future::block_on(host_trust.trust_peer(client.node())).unwrap();
        host_trust
            .enroll_verified_key(
                network,
                client.node(),
                client.fingerprint(),
                client.fingerprint(),
            )
            .unwrap();
        future::block_on(client_trust.trust_peer(host.node())).unwrap();
        client_trust
            .enroll_verified_key(network, host.node(), host.fingerprint(), host.fingerprint())
            .unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(host_path.clone()),
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(Instant::now() < ready_before, "guarded NAP did not bind");
            thread::sleep(Duration::from_millis(5));
        };

        let socket = std::net::TcpStream::connect(endpoint).unwrap();
        let (mut hostile, certificate) = crate::connect_pinned_control_tls(
            socket,
            &client,
            network,
            host_pin,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        hostile
            .exchange_authenticated_hello(
                network,
                &client,
                &certificate,
                control_setup(network, NodeCapabilities::default()).facts,
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        // This is TLS plaintext *after* successful certificate/Hello proof,
        // not an invalid pre-TLS record. Keep the hostile socket alive so a
        // successful same-peer replacement proves the worker retired the
        // malformed generation rather than merely noticing client EOF.
        hostile.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
        hostile.flush().unwrap();

        let retry_until = Instant::now() + Duration::from_secs(4);
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut recovered = loop {
            let socket = std::net::TcpStream::connect(endpoint).unwrap();
            match crate::connect_registered_control_session(
                socket,
                &client,
                host_pin,
                &client_trust,
                control_setup(network, NodeCapabilities::default()),
                Arc::clone(&registry),
            ) {
                Ok(session) => break session,
                Err(error) => {
                    assert!(
                        Instant::now() < retry_until,
                        "running worker did not retire hostile authenticated peer: {error:?}"
                    );
                    thread::sleep(Duration::from_millis(50));
                }
            }
        };
        assert_eq!(recovered.peer(), NodeId::from_bytes([8; 16]));
        recovered
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_err());
        assert_eq!(host_trust.approved_control_pins(network).unwrap().len(), 1);
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            Some(network)
        );

        worker.shutdown().unwrap();
        drop(recovered);
        drop(hostile);
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_worker_rejects_authenticated_replay_scope_and_handshake_frames() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;
        use blueroute_protocol::control::encode_control_frame;
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "blueroute-worker-auth-frame-scope-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let client_path = root.join("client-membership");
        save_host_membership(&host_path);
        save_host_membership(&client_path);
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let host_pin = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint()).unwrap();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let client_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(client_path),
            PeerPinStore::new(root.join("client-pins")),
        );
        // Synthetic fixture pins; actual TLS 1.3 and exporter-bound Hello.
        future::block_on(host_trust.trust_peer(client.node())).unwrap();
        host_trust
            .enroll_verified_key(
                network,
                client.node(),
                client.fingerprint(),
                client.fingerprint(),
            )
            .unwrap();
        future::block_on(client_trust.trust_peer(host.node())).unwrap();
        client_trust
            .enroll_verified_key(network, host.node(), host.fingerprint(), host.fingerprint())
            .unwrap();

        fn write_raw_frame(stream: &mut impl std::io::Write, payload: &[u8]) {
            assert!(payload.len() <= blueroute_protocol::control::MAX_CONTROL_FRAME_BYTES);
            stream
                .write_all(&(payload.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(payload).unwrap();
        }

        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(host_path.clone()),
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(Instant::now() < ready_before, "guarded NAP did not bind");
            thread::sleep(Duration::from_millis(5));
        };

        for attack in 0..14 {
            let socket = std::net::TcpStream::connect(endpoint).unwrap();
            let (mut hostile, certificate) = crate::connect_pinned_control_tls(
                socket,
                &client,
                network,
                host_pin,
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
            let authenticated = hostile
                .exchange_authenticated_hello(
                    network,
                    &client,
                    &certificate,
                    control_setup(network, NodeCapabilities::default()).facts,
                    Instant::now() + Duration::from_secs(5),
                )
                .unwrap();
            let mut envelope = blueroute_protocol::control::ControlEnvelope {
                version: blueroute_protocol::control::CONTROL_VERSION,
                network,
                sender: client.node(),
                session_id: *authenticated.session_id(),
                sequence: 3,
                message: ControlMessage::Heartbeat,
            };
            match attack {
                0 => {
                    // Replay a syntactically valid post-Hello Heartbeat.
                    let frame = encode_control_frame(&envelope).unwrap();
                    hostile.write_all(&frame).unwrap();
                    hostile.write_all(&frame).unwrap();
                }
                1 => {
                    envelope.network = NetworkId::from_bytes([6; 16]);
                    hostile
                        .write_all(&encode_control_frame(&envelope).unwrap())
                        .unwrap();
                }
                2 => {
                    envelope.session_id[0] ^= 1;
                    hostile
                        .write_all(&encode_control_frame(&envelope).unwrap())
                        .unwrap();
                }
                3 => {
                    envelope.message = ControlMessage::Hello {
                        challenge: [1; 16],
                        facts: control_setup(network, NodeCapabilities::default()).facts,
                    };
                    hostile
                        .write_all(&encode_control_frame(&envelope).unwrap())
                        .unwrap();
                }
                4 => {
                    // An approved TLS key does not authorize a different NodeId.
                    envelope.sender = NodeId::from_bytes([10; 16]);
                    hostile
                        .write_all(&encode_control_frame(&envelope).unwrap())
                        .unwrap();
                }
                5 => {
                    // The normal encoder refuses unsupported versions, so
                    // mutate the wire JSON after creating a valid envelope.
                    let mut value = serde_json::to_value(&envelope).unwrap();
                    value["version"] = serde_json::json!(2);
                    write_raw_frame(&mut hostile, &serde_json::to_vec(&value).unwrap());
                }
                6 => {
                    // An authenticated channel cannot extend the message set.
                    let mut value = serde_json::to_value(&envelope).unwrap();
                    value["message"] = serde_json::json!({"type": "route_update"});
                    write_raw_frame(&mut hostile, &serde_json::to_vec(&value).unwrap());
                }
                7 => {
                    // Reject ambiguous duplicate JSON keys at the TLS ingress.
                    let mut payload = serde_json::to_string(&envelope).unwrap();
                    assert!(payload.ends_with('}'));
                    payload.pop();
                    payload.push_str(r#","sequence":4}"#);
                    write_raw_frame(&mut hostile, payload.as_bytes());
                }
                8 => {
                    // A valid frame followed by junk inside the same bounded
                    // length prefix is not an accepted Heartbeat.
                    let mut payload = serde_json::to_vec(&envelope).unwrap();
                    payload.extend_from_slice(b"trailing-garbage");
                    write_raw_frame(&mut hostile, &payload);
                }
                9 => {
                    // A zero-byte body cannot advance authenticated liveness.
                    hostile.write_all(&0_u32.to_be_bytes()).unwrap();
                }
                10 => {
                    // TLS authentication does not make arbitrary bytes UTF-8.
                    write_raw_frame(&mut hostile, &[0xff, 0xfe, 0xfd]);
                }
                11 => {
                    // A valid JSON document with the wrong root type is not
                    // a control envelope.
                    write_raw_frame(&mut hostile, b"[]");
                }
                12 => {
                    // The sequence number must be an integer, not a string.
                    let mut value = serde_json::to_value(&envelope).unwrap();
                    value["sequence"] = serde_json::json!("3");
                    write_raw_frame(&mut hostile, &serde_json::to_vec(&value).unwrap());
                }
                13 => {
                    // An authenticated sender cannot add critical envelope
                    // fields to silently change control-plane semantics.
                    let mut value = serde_json::to_value(&envelope).unwrap();
                    value["route_authority"] = serde_json::json!(true);
                    write_raw_frame(&mut hostile, &serde_json::to_vec(&value).unwrap());
                }
                _ => unreachable!(),
            }
            // The worker may already have rejected the frame and closed the session, so the
            // hostile side's flush can fail with a broken pipe. What is under test is that the
            // bad generation is retired and the same peer can recover, checked below.
            let _ = hostile.flush();

            // Keep the hostile TLS socket open. Same-peer recovery requires
            // the daemon to reject and retire the bad session generation.
            let retry_until = Instant::now() + Duration::from_secs(6);
            let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
            let mut recovered = loop {
                let socket = std::net::TcpStream::connect(endpoint).unwrap();
                match crate::connect_registered_control_session(
                    socket,
                    &client,
                    host_pin,
                    &client_trust,
                    control_setup(network, NodeCapabilities::default()),
                    Arc::clone(&registry),
                ) {
                    Ok(session) => break session,
                    Err(error) => {
                        assert!(
                            Instant::now() < retry_until,
                            "worker did not retire attack {attack} generation: {error:?}"
                        );
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            };
            recovered
                .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
                .unwrap();
            drop(recovered);
            drop(hostile);
            assert_eq!(host_trust.approved_control_pins(network).unwrap().len(), 1);
            let saved = NetworkMembershipStore::new(host_path.clone())
                .load()
                .unwrap();
            assert_eq!(current_network(&saved).unwrap(), Some(network));
            assert!(std::net::TcpListener::bind(endpoint).is_err());
        }

        worker.shutdown().unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_closes_authenticated_worker_session_without_mutating_membership() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::NodeId;

        let root = std::env::temp_dir().join(format!(
            "blueroute-control-worker-auth-shutdown-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let client_path = root.join("client-membership");
        save_host_membership(&host_path);
        save_host_membership(&client_path);
        let host_store = NetworkMembershipStore::new(host_path.clone());
        let client_store = NetworkMembershipStore::new(client_path);
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let host_pin = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint()).unwrap();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let client_trust = DurablePeerTrustOperations::with_peer_pin_store(
            client_store,
            PeerPinStore::new(root.join("client-pins")),
        );

        // These local fixtures emulate completed enrollment. Production can
        // only persist a pin after independent confirmation and genuine TLS.
        future::block_on(host_trust.trust_peer(client.node())).unwrap();
        host_trust
            .enroll_verified_key(
                network,
                client.node(),
                client.fingerprint(),
                client.fingerprint(),
            )
            .unwrap();
        future::block_on(client_trust.trust_peer(host.node())).unwrap();
        client_trust
            .enroll_verified_key(network, host.node(), host.fingerprint(), host.fingerprint())
            .unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let worker = spawn_daemon_control_worker(
            host_store,
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            worker_capabilities(),
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < ready_before,
                "guarded test NAP did not bind"
            );
            thread::sleep(Duration::from_millis(5));
        };
        // A hostile unauthenticated TCP client can send an invalid TLS
        // record. It must get EOF, not crash the worker, retire the NAP
        // listener, or prevent a separately approved peer from connecting.
        use std::io::{Read, Write};
        let mut attacker = std::net::TcpStream::connect(endpoint).unwrap();
        attacker
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        attacker
            .write_all(b"not-a-valid-TLS-client-record")
            .unwrap();
        attacker.shutdown(std::net::Shutdown::Write).unwrap();
        let mut received = [0_u8; 64];
        loop {
            match attacker.read(&mut received) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => panic!("malformed TLS client was not closed: {error}"),
            }
        }
        assert!(
            std::net::TcpListener::bind(endpoint).is_err(),
            "malformed TLS input must not retire the host listener"
        );

        let socket = std::net::TcpStream::connect(endpoint).unwrap();
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut session = crate::connect_registered_control_session(
            socket,
            &client,
            host_pin,
            &client_trust,
            control_setup(network, NodeCapabilities::default()),
            Arc::clone(&registry),
        )
        .expect("real TLS and exporter-bound Hello must authenticate the running worker");
        assert_eq!(session.peer(), NodeId::from_bytes([8; 16]));
        session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(2))
            .unwrap();

        // Stop the daemon worker while an independently pinned mutual-TLS
        // channel is active. Shutdown must close both TLS and the NAP
        // listener without rewriting durable membership or peer pins.
        worker.shutdown().unwrap();
        let read_start = Instant::now();
        let read_until = read_start + Duration::from_secs(4);
        loop {
            let now_ms = 3 + u64::try_from(read_start.elapsed().as_millis()).unwrap();
            match session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                    assert!(
                        Instant::now() < read_until,
                        "the stopped worker kept an authenticated socket alive"
                    );
                }
                Ok(ControlSessionRead::Closed) | Err(_) => break,
            }
        }
        assert!(session.is_terminal());
        assert_eq!(host_trust.approved_control_pins(network).unwrap().len(), 1);
        assert_eq!(
            current_network(&NetworkMembershipStore::new(host_path).load().unwrap()).unwrap(),
            Some(network)
        );
        assert!(
            std::net::TcpListener::bind(endpoint).is_ok(),
            "shutdown must release the host listener"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn running_worker_rejects_unenrolled_tcp_without_changing_membership() {
        use blueroute_core::{
            DisplayName, MembershipRegistry, MembershipState, NetworkMembership, NodeId,
        };
        use std::io::Read;

        let path = std::env::temp_dir().join(format!(
            "blueroute-control-unauthorized-{}",
            std::process::id()
        ));
        let store = NetworkMembershipStore::new(path.clone());
        let network = NetworkId::from_bytes([7; 16]);
        let mut membership =
            NetworkMembership::new(network, DisplayName::new("Test host").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        store.save(&registry).unwrap();

        let verify = NetworkMembershipStore::new(path.clone());
        let operations = Arc::new(HostedNapOperations::default());
        // No durable enrolled key store: an incoming TCP source is not a peer.
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            store,
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not start NAP listener"
            );
            thread::sleep(Duration::from_millis(5));
        };

        let mut untrusted = std::net::TcpStream::connect(endpoint).unwrap();
        untrusted
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(
            untrusted.read(&mut byte).unwrap(),
            0,
            "non-enrolled TCP source must be rejected without a TLS session"
        );
        assert_eq!(
            current_network(&verify.load().unwrap()).unwrap(),
            Some(network)
        );
        assert!(
            std::net::TcpListener::bind(endpoint).is_err(),
            "denying one peer must preserve the owned NAP listener"
        );
        worker.shutdown().unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_ok());
    }

    #[test]
    fn live_listener_retires_when_guarded_nap_ownership_disappears() {
        use blueroute_core::{
            DisplayName, MembershipRegistry, MembershipState, NetworkMembership, NodeId,
        };

        let path = std::env::temp_dir().join(format!(
            "blueroute-control-ownership-loss-{}",
            std::process::id()
        ));
        let store = NetworkMembershipStore::new(path.clone());
        let network = NetworkId::from_bytes([7; 16]);
        let mut membership =
            NetworkMembership::new(network, DisplayName::new("Test host").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        store.save(&registry).unwrap();

        let operations = Arc::new(HostedNapOperations::default());
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            store,
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();

        let wait_until = Instant::now() + Duration::from_secs(2);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < wait_until,
                "NAP test listener was never bound"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert!(std::net::TcpListener::bind(endpoint).is_err());
        operations.ownership_lost.store(true, Ordering::Release);

        let release_deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Ok(probe) = std::net::TcpListener::bind(endpoint) {
                drop(probe);
                break;
            }
            assert!(
                Instant::now() < release_deadline,
                "loss of trusted NAP ownership must retire the bound TCP listener"
            );
            thread::sleep(Duration::from_millis(10));
        }
        // A new guard query must not rebind when the NAP is no longer owned.
        let observed_binds = operations.binds.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(operations.binds.load(Ordering::SeqCst), observed_binds);
        assert!(std::net::TcpListener::bind(endpoint).is_ok());

        // Recovered local ownership requires a fresh independent guarded bind,
        // not revival of a retired listener or its old session generations.
        operations.ownership_lost.store(false, Ordering::Release);
        let recovery_deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if operations.binds.load(Ordering::SeqCst) > observed_binds {
                break;
            }
            assert!(
                Instant::now() < recovery_deadline,
                "restored NAP ownership must permit a new guarded listener"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let recovered = operations.endpoint.lock().unwrap().unwrap();
        assert!(std::net::TcpListener::bind(recovered).is_err());
        worker.shutdown().unwrap();
        assert!(std::net::TcpListener::bind(recovered).is_ok());
    }

    #[test]
    fn durable_leave_retires_stale_nap_listener() {
        use blueroute_core::NodeId;
        let path = std::env::temp_dir().join(format!(
            "blueroute-control-durable-leave-{}",
            std::process::id()
        ));
        save_host_membership(&path);
        let operations = Arc::new(HostedNapOperations::default());
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path.clone()),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(path.clone()),
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        NetworkMembershipStore::new(path.clone())
            .save(&blueroute_core::MembershipRegistry::default())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while std::net::TcpListener::bind(endpoint).is_err() {
            assert!(
                Instant::now() < deadline,
                "durable leave must close listener"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let count = operations.binds.load(Ordering::SeqCst);
        thread::sleep(REBIND_INTERVAL + Duration::from_millis(300));
        assert_eq!(operations.binds.load(Ordering::SeqCst), count);
        save_host_membership(&path);
        let deadline = Instant::now() + Duration::from_secs(4);
        while operations.binds.load(Ordering::SeqCst) == count {
            assert!(Instant::now() < deadline, "restored host must rebind");
            thread::sleep(Duration::from_millis(10));
        }
        let replacement = operations.endpoint.lock().unwrap().unwrap();
        assert!(std::net::TcpListener::bind(replacement).is_err());
        worker.shutdown().unwrap();
        assert!(std::net::TcpListener::bind(replacement).is_ok());
    }

    #[test]
    fn cancellation_during_nap_bind_closes_socket_before_any_observation() {
        use blueroute_core::NodeId;
        use std::sync::mpsc;

        let path = std::env::temp_dir().join(format!(
            "blueroute-worker-cancel-during-bind-{}",
            std::process::id()
        ));
        save_host_membership(&path);
        let operations = Arc::new(HostedNapOperations::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *operations.bind_gate.lock().unwrap() = Some((entered_tx, release_rx));
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path.clone()),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(path),
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();

        // The fake backend owns a real listening socket while its bind
        // future is paused. A cancellation request here is deterministic,
        // not a test of thread scheduling or radio behavior.
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let endpoint = operations.endpoint.lock().unwrap().unwrap();
        assert!(std::net::TcpListener::bind(endpoint).is_err());
        worker.request_stop();
        release_tx.send(()).unwrap();
        worker.shutdown().unwrap();

        assert_eq!(operations.binds.load(Ordering::SeqCst), 1);
        assert_eq!(
            operations.observed_endpoints.load(Ordering::SeqCst),
            0,
            "cancelled bind must not proceed to guarded NAP observation"
        );
        assert!(
            std::net::TcpListener::bind(endpoint).is_ok(),
            "cancellation must release a TCP listener returned by the backend"
        );
    }

    #[test]
    fn running_worker_without_nap_stops_during_long_rebind_park() {
        use blueroute_core::NodeId;

        let path =
            std::env::temp_dir().join(format!("blueroute-control-idle-{}", std::process::id()));
        save_host_membership(&path);
        let operations = Arc::new(NoNapOperations::default());
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path.clone()),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([7; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(path),
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();
        let wait_until = Instant::now() + Duration::from_secs(2);
        while operations.bind_attempts.load(Ordering::SeqCst) == 0 && Instant::now() < wait_until {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(operations.bind_attempts.load(Ordering::SeqCst) > 0);
        let started = Instant::now();
        worker.shutdown().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "cancellation must wake the worker rather than waiting for its 2-second rebind park"
        );
    }

    #[test]
    fn worker_retries_transient_bind_failure_and_shutdown_wakes_rebind_park() {
        use blueroute_core::NodeId;

        let path = std::env::temp_dir().join(format!(
            "blueroute-control-bind-error-{}",
            std::process::id()
        ));
        save_host_membership(&path);
        let operations = Arc::new(NoNapOperations::default());
        operations.fail_bind.store(true, Ordering::SeqCst);
        let trust = Arc::new(DurablePeerTrustOperations::new(
            NetworkMembershipStore::new(path.clone()),
        ));
        let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(path),
            operations.clone(),
            trust,
            local,
            worker_capabilities(),
        )
        .unwrap();

        let first_attempt = Instant::now() + Duration::from_secs(2);
        while operations.bind_attempts.load(Ordering::SeqCst) == 0 && Instant::now() < first_attempt
        {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(operations.bind_attempts.load(Ordering::SeqCst) > 0);
        // The initial error must not permanently poison the worker. A fresh
        // observation after the bounded rebind delay can return no NAP.
        operations.fail_bind.store(false, Ordering::SeqCst);
        let retry_deadline = Instant::now() + Duration::from_secs(4);
        while operations.bind_attempts.load(Ordering::SeqCst) < 2 && Instant::now() < retry_deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            operations.bind_attempts.load(Ordering::SeqCst) >= 2,
            "a transient bind error must not disable later guarded observations"
        );
        let started = Instant::now();
        worker.shutdown().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "shutdown must interrupt the rebind park after recovery"
        );
    }

    #[test]
    fn shutdown_wakes_parked_worker_and_joins_without_waiting_for_poll_timeout() {
        use std::sync::mpsc;

        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&stop);
        let (finished, completion) = mpsc::channel();
        let handle = thread::spawn(move || {
            while !observed.load(Ordering::Acquire) {
                thread::park_timeout(Duration::from_secs(60));
            }
            finished.send(()).unwrap();
        });
        let worker = DaemonControlWorker {
            stop,
            handle: Some(handle),
            peers: AuthenticatedControlPeers::default(),
        };
        worker.shutdown().unwrap();
        completion.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn drop_requests_cancellation_without_joining_daemon_executor() {
        use std::sync::mpsc;

        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&stop);
        let (finished, completion) = mpsc::channel();
        let handle = thread::spawn(move || {
            while !observed.load(Ordering::Acquire) {
                thread::park_timeout(Duration::from_secs(60));
            }
            finished.send(()).unwrap();
        });
        drop(DaemonControlWorker {
            stop,
            handle: Some(handle),
            peers: AuthenticatedControlPeers::default(),
        });
        completion.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn running_worker_refuses_a_peer_over_the_policy_ceiling_until_a_slot_frees() {
        use crate::{PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer};
        use blueroute_core::{CapabilitySource, NodeId};

        let root = std::env::temp_dir().join(format!(
            "blueroute-control-worker-ceiling-{}",
            std::process::id()
        ));
        let host_path = root.join("host-membership");
        let first_path = root.join("first-membership");
        let second_path = root.join("second-membership");
        for path in [&host_path, &first_path, &second_path] {
            save_host_membership(path);
        }
        let network = NetworkId::from_bytes([7; 16]);
        let host = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
        let first = LocalControlTlsIdentity::generate(NodeId::from_bytes([9; 16])).unwrap();
        let second = LocalControlTlsIdentity::generate(NodeId::from_bytes([10; 16])).unwrap();
        let host_node = host.node();
        let host_fingerprint = host.fingerprint();
        let host_trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(host_path.clone()),
            PeerPinStore::new(root.join("host-pins")),
        ));
        let first_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(first_path),
            PeerPinStore::new(root.join("first-pins")),
        );
        let second_trust = DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(second_path),
            PeerPinStore::new(root.join("second-pins")),
        );
        // The host enrolls each joiner the production way: the operator types its key.
        for peer in [&first, &second] {
            assert!(
                future::block_on(host_trust.enroll_peer_key(peer.node(), peer.fingerprint()))
                    .unwrap()
            );
        }
        for trust in [&first_trust, &second_trust] {
            future::block_on(trust.trust_peer(host_node)).unwrap();
            trust
                .enroll_verified_key(network, host_node, host_fingerprint, host_fingerprint)
                .unwrap();
        }

        let operations = Arc::new(HostedNapOperations::default());
        let one_client = NodeCapabilities {
            connection_policy_ceiling: Some(Sourced::new(1, CapabilitySource::Configured)),
            ..NodeCapabilities::default()
        };
        let worker = spawn_daemon_control_worker(
            NetworkMembershipStore::new(host_path),
            operations.clone(),
            Arc::clone(&host_trust),
            host,
            one_client,
        )
        .unwrap();
        let ready_before = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = *operations.endpoint.lock().unwrap() {
                break endpoint;
            }
            assert!(
                Instant::now() < ready_before,
                "guarded test NAP did not bind"
            );
            thread::sleep(Duration::from_millis(5));
        };
        let peers = worker.authenticated_peers();
        let wait_for = |expected: Vec<NodeId>| {
            let by = Instant::now() + Duration::from_secs(3);
            while peers.snapshot() != expected {
                assert!(
                    Instant::now() < by,
                    "peers {:?} != {expected:?}",
                    peers.snapshot()
                );
                thread::sleep(Duration::from_millis(5));
            }
        };
        let expected = PinnedControlTlsPeer::new(network, host_node, host_fingerprint).unwrap();
        fn connect<'a>(
            endpoint: std::net::SocketAddrV4,
            identity: &LocalControlTlsIdentity,
            trust: &'a DurablePeerTrustOperations,
            expected: PinnedControlTlsPeer,
        ) -> RegisteredControlSession<'a> {
            crate::connect_registered_control_session(
                std::net::TcpStream::connect(endpoint).unwrap(),
                identity,
                expected,
                trust,
                control_setup(expected.network(), NodeCapabilities::default()),
                Arc::new(Mutex::new(ControlSessionRegistry::new())),
            )
            .expect("mutual TLS authenticates before the ceiling is checked")
        }
        let closed_within = |session: &mut RegisteredControlSession<'_>, seconds: u64| {
            let started = Instant::now();
            let by = started + Duration::from_secs(seconds);
            loop {
                let now_ms =
                    ACTIVE_CLOCK_BASE_MS + u64::try_from(started.elapsed().as_millis()).unwrap();
                match session.receive_next_with_budget_at(now_ms, Duration::from_millis(80)) {
                    Ok(ControlSessionRead::Frames(frames))
                        if frames
                            .iter()
                            .any(|frame| matches!(frame.message, ControlMessage::Goodbye)) =>
                    {
                        return true;
                    }
                    Ok(ControlSessionRead::Idle | ControlSessionRead::Frames(_)) => {
                        if Instant::now() >= by {
                            return false;
                        }
                    }
                    Ok(ControlSessionRead::Closed) | Err(_) => return true,
                }
            }
        };

        let mut first_session = connect(endpoint, &first, &first_trust, expected);
        wait_for(vec![first.node()]);

        // Over the ceiling: authenticated, then told Goodbye and closed; never listed.
        let mut refused = connect(endpoint, &second, &second_trust, expected);
        assert!(
            closed_within(&mut refused, 4),
            "over-ceiling peer kept its session"
        );
        assert_eq!(peers.snapshot(), vec![first.node()]);
        assert!(!first_session.is_terminal());

        // The first peer leaves; its slot frees and the second is admitted on retry.
        first_session
            .send_goodbye_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        wait_for(Vec::new());
        let mut admitted = connect(endpoint, &second, &second_trust, expected);
        wait_for(vec![second.node()]);
        assert!(
            !closed_within(&mut admitted, 1),
            "a peer under the ceiling was refused"
        );

        worker.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_stops_at_the_policy_ceiling_and_never_counts_a_peer_twice() {
        use blueroute_core::{CapabilitySource, NodeId};
        let node = |value: u8| NodeId::from_bytes([value; 16]);
        let two = Sourced::new(2, CapabilitySource::ConservativeDefault);
        assert_eq!(within_policy_ceiling(Some(&two), [], node(1)), Ok(()));
        assert_eq!(
            within_policy_ceiling(Some(&two), [node(1)], node(2)),
            Ok(())
        );
        assert!(within_policy_ceiling(Some(&two), [node(1), node(2)], node(3)).is_err());
        // An already-admitted peer is not a new client.
        assert_eq!(
            within_policy_ceiling(Some(&two), [node(1), node(2)], node(2)),
            Ok(())
        );
        // Unknown or zero ceilings refuse everyone.
        assert!(within_policy_ceiling(None, [], node(1)).is_err());
        let zero = Sourced::new(0, CapabilitySource::Configured);
        assert!(within_policy_ceiling(Some(&zero), [], node(1)).is_err());
    }

    #[test]
    fn active_tls_clocks_are_per_generation_and_fail_closed_on_regression() {
        let first = Instant::now();
        let second = first + Duration::from_secs(10);
        let check = second + Duration::from_secs(5);
        assert_eq!(session_clock_ms(first, check), Some(15_003));
        assert_eq!(session_clock_ms(second, check), Some(5_003));
        assert_eq!(session_clock_ms(second, second), Some(3));
        assert_eq!(session_clock_ms(second, first), None);
    }

    #[test]
    fn heartbeat_schedule_is_bounded_and_rejects_regressed_time() {
        let sent = Instant::now();
        assert!(!heartbeat_due(sent, sent));
        assert!(!heartbeat_due(
            sent,
            sent + HEARTBEAT_INTERVAL - Duration::from_millis(1)
        ));
        assert!(heartbeat_due(sent, sent + HEARTBEAT_INTERVAL));
        assert!(!heartbeat_due(sent, sent - Duration::from_millis(1)));
    }

    #[test]
    fn provisional_hello_has_no_invented_neighbors_or_segment() {
        let network = NetworkId::from_bytes([9; 16]);
        let setup = control_setup(network, NodeCapabilities::default());
        assert_eq!(setup.network, network);
        assert!(setup.facts.direct_neighbors.is_empty());
        assert_eq!(setup.facts.segment, None);
        assert_eq!(setup.active_deadline.stage(), ConnectionStage::Active);
    }
}

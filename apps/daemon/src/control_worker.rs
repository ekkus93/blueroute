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

use blueroute_core::{NetworkId, NodeCapabilities};
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

/// A daemon-lifetime owner for the NAP control worker.
///
/// Drop requests cancellation and wakes the worker's idle park. Call
/// `shutdown` to also wait for the worker to close all owned TLS sessions.
/// An in-flight bounded handshake or backend observation may delay the join.
pub struct DaemonControlWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DaemonControlWorker {
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
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
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
            )
        })?;
    Ok(DaemonControlWorker {
        stop,
        handle: Some(handle),
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
            match future::block_on(operations.bind_guarded_control_listener()) {
                Ok(Some(bound)) => listener = Some(bound),
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
                    Ok(Some(registered)) => {
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
        thread::park_timeout(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
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
            NodeCapabilities::default(),
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
            NodeCapabilities::default(),
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
            NodeCapabilities::default(),
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
            NodeCapabilities::default(),
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
            NodeCapabilities::default(),
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
            NodeCapabilities::default(),
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
        });
        completion.recv_timeout(Duration::from_secs(1)).unwrap();
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

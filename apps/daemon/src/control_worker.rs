//! P7-004 daemon-owned inbound NAP/TLS listener (software-first).
//!
//! An independent worker keeps synchronous TLS I/O off the D-Bus executor.
//! Only durable enrolled pins can pass TLS and Hello. Until frame/heartbeat
//! processing is wired, bounded leases close sessions proactively. This
//! does not enable production JoinNetwork or prove Bluetooth radio behavior.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use blueroute_core::{NetworkId, NodeCapabilities};
use blueroute_linux::NetworkMembershipStore;
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::session::{ConnectionDeadline, ConnectionStage, ControlSessionRegistry};
use futures_lite::future;

use crate::{
    ApprovedControlSetup, DurablePeerTrustOperations, LocalControlTlsIdentity, NetworkOperations,
    ObservedControlListener, RegisteredControlSession, current_network,
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const REBIND_INTERVAL: Duration = Duration::from_secs(2);
const MAX_UNSUPERVISED_LIFETIME: Duration = Duration::from_secs(15);
const MAX_OWNED_SESSIONS: usize = 32;
const GOODBYE_WRITE_BUDGET: Duration = Duration::from_millis(100);

/// The caller must retain the handle for the daemon lifetime. No active NAP
/// means no listener. A new host is discovered via a fresh guarded bind.
pub fn spawn_daemon_control_worker(
    membership: NetworkMembershipStore,
    operations: Arc<dyn NetworkOperations>,
    trust: Arc<DurablePeerTrustOperations>,
    local: LocalControlTlsIdentity,
    capabilities: NodeCapabilities,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("blueroute-control-tls".to_owned())
        .spawn(move || run_worker(membership, operations, trust, local, capabilities))
}

fn control_setup(network: NetworkId, capabilities: NodeCapabilities) -> ApprovedControlSetup {
    // Session reads are not yet daemon-wired: the actual lifetime is limited
    // by the wall-clock lease below, not this provisional Active deadline.
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

// Reject elapsed or regressed leases. No late registration can extend a
// different peer's lifetime; a replacement exporter has a distinct ID.
fn lease_is_current(admitted: Instant, now: Instant) -> bool {
    now.checked_duration_since(admitted)
        .is_some_and(|elapsed| elapsed < MAX_UNSUPERVISED_LIFETIME)
}

fn run_worker(
    membership: NetworkMembershipStore,
    operations: Arc<dyn NetworkOperations>,
    trust: Arc<DurablePeerTrustOperations>,
    local: LocalControlTlsIdentity,
    capabilities: NodeCapabilities,
) {
    let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
    let mut listener: Option<ObservedControlListener> = None;
    let mut sessions: Vec<RegisteredControlSession<'_>> = Vec::new();
    let mut registered_at: BTreeMap<[u8; 16], Instant> = BTreeMap::new();
    loop {
        // Each actual TLS generation owns its own strict deadline. A newly
        // admitted peer cannot borrow time from another active peer.
        let now = Instant::now();
        sessions.retain(|session| {
            let admitted = registered_at.get(session.session_id()).copied();
            if admitted.is_some_and(|start| lease_is_current(start, now)) {
                return true;
            }
            // Only a known expired lease can send a best-effort Goodbye.
            // Unknown or regressed leases close silently and fail closed.
            if admitted.is_some_and(|start| {
                now.checked_duration_since(start)
                    .is_some_and(|elapsed| elapsed >= MAX_UNSUPERVISED_LIFETIME)
            }) {
                if let Some(deadline) = Instant::now().checked_add(GOODBYE_WRITE_BUDGET) {
                    let _ = session.send_goodbye_until(deadline);
                }
            }
            false
        });
        registered_at.retain(|session_id, _| {
            sessions
                .iter()
                .any(|session| session.session_id() == session_id)
        });
        if listener.is_none() {
            sessions.clear();
            registered_at.clear();
            match future::block_on(operations.bind_guarded_control_listener()) {
                Ok(Some(bound)) => listener = Some(bound),
                Ok(None) => {}
                Err(error) => eprintln!("BlueRoute NAP listener bind denied: {:?}", error.kind()),
            }
            if listener.is_none() {
                thread::sleep(REBIND_INTERVAL);
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
            listener = None;
            thread::sleep(REBIND_INTERVAL);
            continue;
        }
        // Revocation may have removed a session during the fresh durability
        // recheck. Release its matching provisional lease as well.
        registered_at.retain(|session_id, _| {
            sessions
                .iter()
                .any(|session| session.session_id() == session_id)
        });
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
                        registered_at.insert(*registered.session_id(), Instant::now());
                        sessions.push(registered);
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("BlueRoute inbound TLS denied: {:?}", error.kind()),
                }
            } else {
                sessions.clear();
                registered_at.clear();
                listener = None;
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisional_tls_leases_are_per_generation_and_fail_closed_at_deadline() {
        let first = Instant::now();
        let second = first + Duration::from_secs(10);
        let check = first + MAX_UNSUPERVISED_LIFETIME;
        assert!(!lease_is_current(first, check));
        assert!(lease_is_current(second, check));
        assert!(lease_is_current(second, second));
        assert!(!lease_is_current(second, first));
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

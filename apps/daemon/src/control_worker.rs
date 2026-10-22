//! P7-004 daemon-owned inbound NAP/TLS listener (software-first).
//!
//! An independent worker keeps synchronous TLS I/O off the D-Bus executor.
//! Only durable enrolled pins can pass TLS and Hello. Active sessions poll
//! replay-checked Heartbeat/Goodbye under bounded per-peer read slices.
//! This does not enable production JoinNetwork or prove Bluetooth radio behavior.
use std::collections::BTreeMap;
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
) {
    let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
    let mut listener: Option<ObservedControlListener> = None;
    let mut sessions: Vec<RegisteredControlSession<'_>> = Vec::new();
    let mut registered_at: BTreeMap<[u8; 16], Instant> = BTreeMap::new();
    let mut heartbeat_sent_at: BTreeMap<[u8; 16], Instant> = BTreeMap::new();
    loop {
        // Read each authenticated generation for at most one short slice,
        // allowing other peers and fresh NAP observations to make progress.
        // The ingress layer enforces the full idle deadline, replay and pins.
        sessions.retain_mut(|session| {
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
        if listener.is_none() {
            sessions.clear();
            registered_at.clear();
            heartbeat_sent_at.clear();
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
            heartbeat_sent_at.clear();
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
        heartbeat_sent_at.retain(|session_id, _| {
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
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

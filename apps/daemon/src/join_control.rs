//! Production body of the supervised join control session (P6-004, D3 + D4).
//!
//! Runs on the worker thread owned by `ControlSupervisors`: dial the planned NAP endpoint from the
//! owned PANU address with the operator-confirmed host pin, authenticate over mutual TLS 1.3 with
//! the exporter-bound Hello, report establishment, then revalidate ownership and send heartbeats
//! until asked to stop. Every authorization decision goes through the pinned-TLS checks, which
//! for a node in no network accept only the recorded pending host (see `peer_trust`).
//!
//! This talks to real BlueZ, NetworkManager and TCP, so it has no hardware-free test; the
//! lifecycle around it is covered with fakes in `control_supervisor`. It is reachable only
//! through `LinuxJoinRuntime`, which stays closed until hardware acceptance (P6-004).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blueroute_core::{CoreError, Ipv4AddressPool, NetworkId, NodeCapabilities};
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::session::{ConnectionDeadline, ConnectionStage, ControlSessionRegistry};

use crate::{
    ApprovedControlSetup, DurablePeerTrustOperations, Established, JoinIpLease, JoinPanLink,
    LocalControlTlsIdentity, PanuControlRoute, PinnedControlTlsPeer,
    connect_registered_panu_control_session, run_registered_panu_supervision_until_stopped,
};

const HANDSHAKE_BUDGET: Duration = Duration::from_secs(5);

/// Everything the worker needs besides the per-join link, lease and host pin.
#[derive(Clone)]
pub struct JoinControlConfig {
    pub local: Arc<LocalControlTlsIdentity>,
    pub trust: Arc<DurablePeerTrustOperations>,
    pub registry: Arc<Mutex<ControlSessionRegistry>>,
    pub capabilities: NodeCapabilities,
}

fn setup(network: NetworkId, capabilities: NodeCapabilities) -> ApprovedControlSetup {
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
        handshake_deadline: Instant::now() + HANDSHAKE_BUDGET,
        active_deadline,
    }
}

/// Worker-thread body. Returns when stopped (`Ok`) or when authentication or an ownership check
/// fails (`Err`); the session is always closed on the way out by dropping its owner.
pub fn run_join_control(
    config: &JoinControlConfig,
    expected: PinnedControlTlsPeer,
    link: &JoinPanLink,
    lease: &JoinIpLease,
    pool: Ipv4AddressPool,
    stop: &AtomicBool,
    established: &Established,
) -> Result<(), CoreError> {
    let route = || PanuControlRoute { link, lease, pool };
    let mut session = match connect_registered_panu_control_session(
        route(),
        &config.local,
        expected,
        &config.trust,
        setup(expected.network(), config.capabilities.clone()),
        config.registry.clone(),
    ) {
        Ok(session) => session,
        Err(error) => {
            established.report(Err(error.clone()));
            return Err(error);
        }
    };
    established.report(Ok(()));
    run_registered_panu_supervision_until_stopped(
        &mut session,
        route(),
        &config.trust,
        &expected,
        stop,
    )
}

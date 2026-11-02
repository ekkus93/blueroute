//! Outbound PANU control bootstrap, bound to an independently owned IPv4 lease.
//!
//! The endpoint comes from the deterministic network plan, never a Bluetooth
//! advertisement or a peer-provided address. An outbound TCP socket remains
//! untrusted until durable approval, pinned TLS 1.3, exporter-bound Hello and
//! registry admission all succeed. This seam does not enable JoinNetwork:
//! production PANU discovery, pre-join enrollment and reconnect are still open.

use std::net::{IpAddr, Shutdown, SocketAddr, SocketAddrV4, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use blueroute_core::{CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId};
use blueroute_protocol::session::ControlSessionRegistry;

use crate::{
    ApprovedControlSetup, CONTROL_TCP_PORT, DurablePeerTrustOperations, JoinIpLease, JoinPanLink,
    LocalControlTlsIdentity, PinnedControlTlsPeer, RegisteredControlSession,
    connect_registered_control_session,
};

/// Resolve the exact NAP TCP target only from a verified PANU lease.
///
/// This guard must be applied again at the outbound dial boundary even when
/// the join coordinator previously checked the lease: callers must not
/// substitute a foreign interface, network, or IP between setup and TLS.
pub fn planned_panu_control_endpoint(
    network: NetworkId,
    link: &JoinPanLink,
    lease: &JoinIpLease,
    pool: Ipv4AddressPool,
) -> Result<SocketAddrV4, CoreError> {
    if network == NetworkId::from_bytes([0; 16]) {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "zero NetworkId cannot select a PANU control endpoint",
        ));
    }
    let plan = Ipv4StarAddressPlan::for_network(network, pool)?;
    if lease.address().owner != network || lease.interface() != link.interface() {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "PANU control lease has foreign network or interface ownership",
        ));
    }
    if lease.address().prefix != plan.first_client {
        return Err(CoreError::new(
            ErrorKind::AddressConflict,
            "PANU control lease does not match the deterministic client address",
        ));
    }
    let IpAddr::V4(host) = plan.host.address else {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "PANU control target must be IPv4",
        ));
    };
    Ok(SocketAddrV4::new(host, CONTROL_TCP_PORT))
}

/// PANU route provenance supplied by the join runtime, not a remote peer.
/// This is revalidated at every outbound dial.
pub struct PanuControlRoute<'a> {
    pub link: &'a JoinPanLink,
    pub lease: &'a JoinIpLease,
    pub pool: Ipv4AddressPool,
}

/// Dial the planned NAP endpoint and return a registered, authenticated
/// control session, never an unverified TCP socket.
///
/// Durable peer approval and the exact enrolled SPKI pin are checked *before*
/// dialing. The connected socket must also use the owned PANU lease's source
/// address. TLS, exporter-bound Hello and active registration independently
/// recheck identity and approval. No caller may treat a successful TCP dial
/// alone as network membership.
///
/// Linux routing can change after a source-address check; live PAN ownership
/// observation, link-loss supervision and joiner enrollment are still required
/// before wiring this into production JoinNetwork.
pub fn connect_registered_panu_control_session<'a>(
    route: PanuControlRoute<'_>,
    local: &LocalControlTlsIdentity,
    expected: PinnedControlTlsPeer,
    trust: &'a DurablePeerTrustOperations,
    setup: ApprovedControlSetup,
    registry: Arc<Mutex<ControlSessionRegistry>>,
) -> Result<RegisteredControlSession<'a>, CoreError> {
    let endpoint = planned_panu_control_endpoint(setup.network, route.link, route.lease, route.pool)?;
    if expected.network() != setup.network || expected.node() == local.node() {
        return Err(CoreError::new(
            ErrorKind::AuthenticationFailed,
            "outbound control pin is not scoped to the requested remote peer and network",
        ));
    }
    trust.require_approved_pinned_tls_peer(
        setup.network,
        expected.node(),
        expected.fingerprint(),
    )?;
    let budget = setup
        .handshake_deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            CoreError::new(
                ErrorKind::AuthenticationFailed,
                "outbound control handshake deadline expired before TCP dial",
            )
        })?;
    let socket =
        TcpStream::connect_timeout(&SocketAddr::V4(endpoint), budget).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "unable to dial the planned NAP control endpoint",
                error.to_string(),
            )
        })?;
    let source = socket.local_addr().map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::PanFailure,
            "unable to inspect outbound PANU control source address",
            error.to_string(),
        )
    })?;
    if source.ip() != route.lease.address().prefix.address {
        let _ = socket.shutdown(Shutdown::Both);
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "outbound control TCP source does not match the owned PANU address",
        ));
    }
    connect_registered_control_session(socket, local, expected, trust, setup, registry).map_err(
        |error| {
            CoreError::with_diagnostic(
                ErrorKind::AuthenticationFailed,
                "outbound PANU control TLS/Hello/registry admission failed",
                error.to_string(),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_linux::{
        InterfaceAddress, NetworkInterfaceHandle, NetworkMembershipStore, PanAttachment, PanRole,
        PeerHandle,
    };
    use blueroute_protocol::control::HelloFacts;
    use blueroute_protocol::session::{ConnectionDeadline, ConnectionStage};

    fn fixtures(network: NetworkId) -> (JoinPanLink, JoinIpLease, Ipv4AddressPool) {
        let pool = Ipv4AddressPool::default();
        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let interface = NetworkInterfaceHandle::new("bnep-test").unwrap();
        let link = JoinPanLink::new(
            peer.clone(),
            PanAttachment {
                role: PanRole::Panu,
                interface: interface.clone(),
                peer: Some(peer),
            },
        )
        .unwrap();
        let plan = Ipv4StarAddressPlan::for_network(network, pool).unwrap();
        let lease = JoinIpLease::new(InterfaceAddress {
            interface,
            prefix: plan.first_client,
            owner: network,
        });
        (link, lease, pool)
    }

    #[test]
    fn planned_target_is_exact_host_port_for_owned_panu_lease() {
        let network = NetworkId::from_bytes([0x4a; 16]);
        let (link, lease, pool) = fixtures(network);
        let target = planned_panu_control_endpoint(network, &link, &lease, pool).unwrap();
        let plan = Ipv4StarAddressPlan::for_network(network, pool).unwrap();
        assert_eq!(IpAddr::V4(*target.ip()), plan.host.address);
        assert_eq!(target.port(), CONTROL_TCP_PORT);
    }

    #[test]
    fn foreign_or_stale_panu_lease_never_selects_a_control_target() {
        let network = NetworkId::from_bytes([0x4b; 16]);
        let other = NetworkId::from_bytes([0x4c; 16]);
        let (link, lease, pool) = fixtures(network);
        assert!(
            planned_panu_control_endpoint(NetworkId::from_bytes([0; 16]), &link, &lease, pool)
                .is_err()
        );
        assert!(planned_panu_control_endpoint(other, &link, &lease, pool).is_err());

        let wrong_owner = JoinIpLease::new(InterfaceAddress {
            owner: other,
            ..lease.address().clone()
        });
        assert!(planned_panu_control_endpoint(network, &link, &wrong_owner, pool).is_err());

        let wrong_interface = JoinIpLease::new(InterfaceAddress {
            interface: NetworkInterfaceHandle::new("bnep-foreign").unwrap(),
            ..lease.address().clone()
        });
        assert!(planned_panu_control_endpoint(network, &link, &wrong_interface, pool).is_err());

        let other_plan = Ipv4StarAddressPlan::for_network(other, pool).unwrap();
        let wrong_ip = JoinIpLease::new(InterfaceAddress {
            prefix: other_plan.first_client,
            ..lease.address().clone()
        });
        assert!(planned_panu_control_endpoint(network, &link, &wrong_ip, pool).is_err());
    }

    #[test]
    fn wrong_network_pin_is_rejected_before_any_tcp_dial() {
        let network = NetworkId::from_bytes([0x4d; 16]);
        let other = NetworkId::from_bytes([0x4e; 16]);
        let (link, lease, pool) = fixtures(network);
        let local =
            LocalControlTlsIdentity::generate(blueroute_core::NodeId::from_bytes([1; 16])).unwrap();
        let remote =
            LocalControlTlsIdentity::generate(blueroute_core::NodeId::from_bytes([2; 16])).unwrap();
        let expected =
            PinnedControlTlsPeer::new(other, remote.node(), remote.fingerprint()).unwrap();
        let trust = DurablePeerTrustOperations::new(NetworkMembershipStore::new(
            std::env::temp_dir().join("blueroute-outbound-test-nonexistent-membership"),
        ));
        let mut deadline = ConnectionDeadline::new(0);
        deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
        deadline.advance(ConnectionStage::Hello, 2).unwrap();
        deadline.advance(ConnectionStage::Active, 3).unwrap();
        let setup = ApprovedControlSetup {
            network,
            facts: HelloFacts {
                software_version: "outbound-test".to_owned(),
                capabilities: blueroute_core::NodeCapabilities::default(),
                segment: None,
                direct_neighbors: Vec::new(),
            },
            handshake_deadline: Instant::now() + std::time::Duration::from_secs(1),
            active_deadline: deadline,
        };
        let error = connect_registered_panu_control_session(
            PanuControlRoute {
                link: &link,
                lease: &lease,
                pool,
            },
            &local,
            expected,
            &trust,
            setup,
            Arc::new(Mutex::new(ControlSessionRegistry::new())),
        )
        .err()
        .expect("wrong network pin must be denied before dialing");
        assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
    }
}

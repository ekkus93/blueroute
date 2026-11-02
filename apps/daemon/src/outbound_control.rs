//! Outbound PANU control bootstrap, bound to an independently owned IPv4 lease.
//!
//! The endpoint comes from the deterministic network plan, never a Bluetooth
//! advertisement or a peer-provided address. An outbound TCP socket remains
//! untrusted until durable approval, pinned TLS 1.3, exporter-bound Hello and
//! registry admission all succeed. This seam does not enable JoinNetwork:
//! production PANU discovery, pre-join enrollment and reconnect are still open.

use std::net::{IpAddr, Shutdown, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use blueroute_core::{CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId};
use blueroute_linux::{
    ActiveIpv4Prefix, BluezBackend, InterfaceAddress, IpNetworkBackend,
    IpNetworkObservationBackend, NetworkManagerBackend, connect_tcp_from_ipv4_on_device,
};
use blueroute_protocol::session::ControlSessionRegistry;
use futures_lite::future;

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

/// Require a fresh NetworkManager ownership snapshot *and* a kernel-facing
/// active-prefix observation for the exact PANU lease. A configured but inactive
/// profile, a duplicate source address on another interface or network, or
/// unknown interface provenance must never authorize a TCP dial.
/// This is a pre-dial guard, not a substitute
/// for continuous link supervision or binding the socket to a device.
fn require_live_panu_lease(
    lease: &JoinIpLease,
    owned: &[InterfaceAddress],
    active: &[ActiveIpv4Prefix],
) -> Result<(), CoreError> {
    if !owned.iter().any(|address| address == lease.address()) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "PANU control source address is not currently owned by BlueRoute",
        ));
    }
    // A matching lease does not prove exclusive ownership: the same address
    // may also be assigned to a foreign network or interface. Reject such
    // ambiguous snapshots rather than allowing a source-bound SYN to escape
    // through an unintended route. Compare the host IP, not the full prefix:
    // an overlapping /24 and /32 can assign the same IPv4 source address.
    if owned.iter().any(|address| {
        address.prefix.address == lease.address().prefix.address && address != lease.address()
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "PANU control source address has conflicting NetworkManager ownership",
        ));
    }
    if !active.iter().any(|prefix| {
        prefix.interface.as_ref() == Some(lease.interface())
            && prefix.prefix == lease.address().prefix
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "PANU control source address is not active on the owned interface",
        ));
    }
    // The expected interface may be present alongside an unexpected (or
    // unidentifiable) interface with the same address. Presence alone is not
    // sufficient evidence of a unique PANU source.
    if active.iter().any(|prefix| {
        prefix.prefix.address == lease.address().prefix.address
            && (prefix.interface.as_ref() != Some(lease.interface())
                || prefix.prefix != lease.address().prefix)
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "PANU control source address has ambiguous active interface or prefix",
        ));
    }
    Ok(())
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
/// The socket is additionally bound to the PANU device before connecting;
/// link-loss supervision and joiner enrollment are still required
/// before wiring this into production JoinNetwork.
pub fn connect_registered_panu_control_session<'a>(
    route: PanuControlRoute<'_>,
    local: &LocalControlTlsIdentity,
    expected: PinnedControlTlsPeer,
    trust: &'a DurablePeerTrustOperations,
    setup: ApprovedControlSetup,
    registry: Arc<Mutex<ControlSessionRegistry>>,
) -> Result<RegisteredControlSession<'a>, CoreError> {
    let endpoint =
        planned_panu_control_endpoint(setup.network, route.link, route.lease, route.pool)?;
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
    if setup.handshake_deadline <= Instant::now() {
        return Err(CoreError::new(
            ErrorKind::AuthenticationFailed,
            "outbound control handshake deadline expired before TCP dial",
        ));
    }
    // A remembered JoinIpLease alone is not proof that the source address
    // remains applied to the PANU interface. Fail closed before any SYN if
    // NetworkManager no longer owns it or the active interface changed.
    let backend = future::block_on(NetworkManagerBackend::connect_system())?;
    let owned = future::block_on(backend.addresses())?;
    let active = future::block_on(backend.active_ipv4_prefixes_with_interfaces())?;
    require_live_panu_lease(route.lease, &owned, &active)?;
    // A stale NetworkManager address does not establish a live Bluetooth PANU.
    // Re-observe the exact BlueZ peer/interface before allowing any TCP SYN.
    // Do this after potentially blocking NetworkManager observations so a
    // detached or replaced PANU cannot be accepted from the old join handle.
    let bluez = future::block_on(BluezBackend::connect_system())?;
    future::block_on(bluez.require_matching_panu(route.link.attachment()))?;
    // NetworkManager and BlueZ observations may block. Approval or the enrolled key
    // could have been revoked while those backend calls were pending. Recheck
    // durable authorization immediately before allowing any TCP SYN.
    trust.require_approved_pinned_tls_peer(
        setup.network,
        expected.node(),
        expected.fingerprint(),
    )?;

    let IpAddr::V4(source_ip) = route.lease.address().prefix.address else {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "outbound PANU control requires an owned IPv4 source",
        ));
    };
    // Bind the source *before* connect. A post-connect source-address check
    // alone could already have sent an untrusted SYN over another interface.
    let socket =
        connect_tcp_from_ipv4_on_device(
            source_ip,
            endpoint,
            route.link.interface(),
            setup.handshake_deadline,
        ).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "unable to dial the planned NAP control endpoint from the owned PANU address and device",
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
    let network = setup.network;
    let peer = expected.node();
    let fingerprint = expected.fingerprint();
    let deadline = setup.handshake_deadline;
    let session =
        connect_registered_control_session(socket, local, expected, trust, setup, registry)
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::AuthenticationFailed,
                    "outbound PANU control TLS/Hello/registry admission failed",
                    error.to_string(),
                )
            })?;

    // A PANU can disappear or be replaced during TLS/Hello. The pre-dial
    // snapshot cannot authorize retaining an authenticated session after that
    // transition. Dropping the owner on any failure closes the TLS socket and
    // retires its exact registry generation.
    retain_after_post_auth_checks(session, || {
        if Instant::now() >= deadline {
            return Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "outbound PANU control deadline expired before post-authentication validation",
            ));
        }
        let backend = future::block_on(NetworkManagerBackend::connect_system())?;
        let owned = future::block_on(backend.addresses())?;
        let active = future::block_on(backend.active_ipv4_prefixes_with_interfaces())?;
        require_live_panu_lease(route.lease, &owned, &active)?;
        let bluez = future::block_on(BluezBackend::connect_system())?;
        future::block_on(bluez.require_matching_panu(route.link.attachment()))?;
        // The backend queries can block, and peer authorization can be
        // revoked during them. Recheck the exact enrolled key last.
        trust.require_approved_pinned_tls_peer(network, peer, fingerprint)?;
        if Instant::now() >= deadline {
            return Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "outbound PANU control deadline expired during post-authentication validation",
            ));
        }
        Ok(())
    })
}

/// Keep a registered transport only after all post-handshake ownership and
/// authorization checks succeed. Error unwinding drops the registered owner,
/// which shuts down its socket before retiring the session generation.
fn retain_after_post_auth_checks<T>(
    session: T,
    validate: impl FnOnce() -> Result<(), CoreError>,
) -> Result<T, CoreError> {
    validate()?;
    Ok(session)
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

    #[test]
    fn post_auth_validation_drops_registered_owner_on_loss_or_revocation() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Owner(Arc<AtomicUsize>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        for kind in [ErrorKind::PanFailure, ErrorKind::AuthenticationFailed] {
            let dropped = Arc::new(AtomicUsize::new(0));
            let result = retain_after_post_auth_checks(Owner(Arc::clone(&dropped)), || {
                Err(CoreError::new(
                    kind,
                    "injected post-handshake ownership failure",
                ))
            });
            assert!(result.is_err());
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }

        let dropped = Arc::new(AtomicUsize::new(0));
        let owner = retain_after_post_auth_checks(Owner(Arc::clone(&dropped)), || Ok(()))
            .expect("valid post-authentication checks retain the owner");
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        drop(owner);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

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
    fn outbound_lease_requires_live_address_on_the_exact_owned_interface() {
        let network = NetworkId::from_bytes([0x51; 16]);
        let (_link, lease, _pool) = fixtures(network);
        let owned = vec![lease.address().clone()];
        let active = vec![ActiveIpv4Prefix {
            interface: Some(lease.interface().clone()),
            prefix: lease.address().prefix,
        }];
        assert!(require_live_panu_lease(&lease, &owned, &active).is_ok());

        assert!(require_live_panu_lease(&lease, &[], &active).is_err());
        assert!(require_live_panu_lease(&lease, &owned, &[]).is_err());
        assert!(
            require_live_panu_lease(
                &lease,
                &owned,
                &[ActiveIpv4Prefix {
                    interface: None,
                    prefix: lease.address().prefix,
                }],
            )
            .is_err()
        );
        assert!(
            require_live_panu_lease(
                &lease,
                &owned,
                &[ActiveIpv4Prefix {
                    interface: Some(NetworkInterfaceHandle::new("bnep-foreign").unwrap()),
                    prefix: lease.address().prefix,
                }],
            )
            .is_err()
        );
        assert!(
            require_live_panu_lease(
                &lease,
                &[InterfaceAddress {
                    owner: NetworkId::from_bytes([0x52; 16]),
                    ..lease.address().clone()
                }],
                &active,
            )
            .is_err()
        );
    }

    #[test]
    fn ambiguous_duplicate_panu_source_fails_closed_even_when_expected_lease_is_present() {
        let network = NetworkId::from_bytes([0x53; 16]);
        let (link, lease, _pool) = fixtures(network);
        let owned = vec![lease.address().clone()];
        let active = vec![ActiveIpv4Prefix {
            interface: Some(link.interface().clone()),
            prefix: lease.address().prefix,
        }];
        assert!(require_live_panu_lease(&lease, &owned, &active).is_ok());

        let foreign_interface = NetworkInterfaceHandle::new("bnep-foreign").unwrap();
        let mut duplicate_owned = owned.clone();
        duplicate_owned.push(InterfaceAddress {
            interface: foreign_interface.clone(),
            ..lease.address().clone()
        });
        assert_eq!(
            require_live_panu_lease(&lease, &duplicate_owned, &active)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );

        let mut foreign_owner = owned.clone();
        foreign_owner.push(InterfaceAddress {
            owner: NetworkId::from_bytes([0x54; 16]),
            ..lease.address().clone()
        });
        assert_eq!(
            require_live_panu_lease(&lease, &foreign_owner, &active)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );

        for interface in [Some(foreign_interface), None] {
            let mut ambiguous_active = active.clone();
            ambiguous_active.push(ActiveIpv4Prefix {
                interface,
                prefix: lease.address().prefix,
            });
            assert_eq!(
                require_live_panu_lease(&lease, &owned, &ambiguous_active)
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidState
            );
        }

        // A duplicate host IP with a different subnet mask is still a
        // duplicate source address. The old whole-IpPrefix comparison would
        // incorrectly accept both of these ambiguous snapshots.
        let overlapping_prefix =
            blueroute_core::IpPrefix::new(lease.address().prefix.address, 32).unwrap();
        assert_ne!(overlapping_prefix, lease.address().prefix);
        let mut different_mask_owned = owned.clone();
        different_mask_owned.push(InterfaceAddress {
            interface: NetworkInterfaceHandle::new("bnep-foreign-mask").unwrap(),
            prefix: overlapping_prefix,
            owner: network,
        });
        assert_eq!(
            require_live_panu_lease(&lease, &different_mask_owned, &active)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        let mut different_mask_active = active.clone();
        different_mask_active.push(ActiveIpv4Prefix {
            interface: Some(NetworkInterfaceHandle::new("bnep-foreign-mask").unwrap()),
            prefix: overlapping_prefix,
        });
        assert_eq!(
            require_live_panu_lease(&lease, &owned, &different_mask_active)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );

        // Even if the duplicate is on the *same* PANU interface, an
        // additional /32 assignment of the same host IP is not the exact
        // exclusive lease we approved. The active observation must fail
        // closed instead of accepting the first matching /24 entry.
        let mut different_mask_same_interface = active.clone();
        different_mask_same_interface.push(ActiveIpv4Prefix {
            interface: Some(link.interface().clone()),
            prefix: overlapping_prefix,
        });
        assert_eq!(
            require_live_panu_lease(&lease, &owned, &different_mask_same_interface)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
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

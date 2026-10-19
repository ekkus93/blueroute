//! Host-address binding boundary for the P7 TCP-over-PAN listener.
//!
//! This checks the deterministic NAP IPv4 endpoint against independently
//! observed NetworkManager-owned addresses. It does not authenticate a peer,
//! begin accepting sockets, activate JoinNetwork, or prove Bluetooth traffic.

use std::io::ErrorKind as IoErrorKind;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};

use blueroute_core::{CoreError, DaemonConfig, ErrorKind, Ipv4StarAddressPlan, NetworkId};
use blueroute_linux::{InterfaceAddress, NetworkInterfaceHandle};

/// Reserved daemon-to-daemon control port from P7-001.
pub const CONTROL_TCP_PORT: u16 = 46071;

/// Derive the only permitted local NAP control socket address.
///
/// The caller must use a fresh `IpNetworkBackend::addresses()` observation
/// from the active BlueRoute-owned host backend; neither peer advertisements,
/// claimed NodeIds, Bluetooth MACs nor source IPs are authorization evidence.
/// An empty or ambiguous observation is never a reason to bind to 0.0.0.0.
pub fn owned_nap_control_address(
    network: NetworkId,
    config: &DaemonConfig,
    observed: &[InterfaceAddress],
) -> Result<SocketAddrV4, CoreError> {
    if network == NetworkId::from_bytes([0; 16]) {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "invalid zero control network",
        ));
    }
    config.validate()?;
    let plan = Ipv4StarAddressPlan::for_network(network, config.ipv4_address_pool)?;
    let network_string = network.to_string();
    let interface = NetworkInterfaceHandle::new(format!("brb-{}", &network_string[..8]))?;
    let IpAddr::V4(host_ip) = plan.host.address else {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "NAP control host is not IPv4",
        ));
    };

    // Linux binds a TCP socket to an IP, not to the interface provenance of
    // its NetworkManager record. Refuse an IP also attributed to any other
    // interface/network instead of guessing which interface will receive it.
    if observed.iter().any(|address| {
        address.prefix.address == plan.host.address
            && (address.owner != network || address.interface != interface)
    }) {
        return Err(CoreError::new(
            ErrorKind::AddressConflict,
            "NAP control address has ambiguous interface ownership",
        ));
    }
    // A bridge attributed to another logical network is not exclusively
    // owned, even if the foreign address differs from our planned host IP.
    // Do not rely on Linux TCP bind to distinguish the network provenance.
    if observed
        .iter()
        .any(|address| address.interface == interface && address.owner != network)
    {
        return Err(CoreError::new(
            ErrorKind::AddressConflict,
            "NAP control bridge has foreign network ownership",
        ));
    }
    // The selected bridge must not report a conflicting BlueRoute-owned
    // address, even if the expected address is also present.
    if observed.iter().any(|address| {
        address.owner == network && address.interface == interface && address.prefix != plan.host
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "NAP control bridge address differs from the deterministic plan",
        ));
    }
    let found = observed.iter().any(|address| {
        address.owner == network && address.interface == interface && address.prefix == plan.host
    });
    if !found {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "active BlueRoute-owned NAP host address is not observed",
        ));
    }
    Ok(SocketAddrV4::new(host_ip, CONTROL_TCP_PORT))
}

/// Bind only after the local host backend reports the exact owned address.
/// Callers must subsequently accept connections using the independently
/// enrolled, mutually authenticated P7 control-session entry points; this
/// function deliberately does not accept any sockets or authorize membership.
pub fn bind_owned_nap_control_listener(
    network: NetworkId,
    config: &DaemonConfig,
    observed: &[InterfaceAddress],
) -> Result<TcpListener, CoreError> {
    let address = owned_nap_control_address(network, config, observed)?;
    TcpListener::bind(address).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::PanFailure,
            "failed to bind BlueRoute-owned NAP control address",
            error.to_string(),
        )
    })
}

/// Own a nonblocking control listener and retire it when the independently
/// observed NAP network or endpoint changes. This only hands out *untrusted*
/// TCP sockets; callers MUST run pinned mutual TLS, Hello and durable approval
/// before accepting any peer claim or network membership.
///
/// The caller must obtain the current endpoint from a fresh guarded
/// NetworkOperations::observed_owned_control_endpoint() observation and the
/// current NetworkId from trusted local membership state before *each* poll.
/// A snapshot can change immediately after a successful poll: the caller must
/// revalidate live ownership before TLS/session activation, and on teardown
/// must close active authenticated sessions independently.
pub struct ObservedControlListener {
    network: NetworkId,
    endpoint: SocketAddrV4,
    listener: Option<TcpListener>,
}

impl ObservedControlListener {
    /// Takes ownership only of an already bound, non-wildcard IPv4 listener
    /// at the exact endpoint supplied by a fresh independent NAP observation.
    /// For production, obtain the socket via the guarded owned-NAP binder.
    pub fn new(
        network: NetworkId,
        observed_endpoint: SocketAddrV4,
        listener: TcpListener,
    ) -> Result<Self, CoreError> {
        if network == NetworkId::from_bytes([0; 16]) {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "control listener requires a nonzero local network",
            ));
        }
        let local = listener.local_addr().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "cannot inspect bound NAP control listener",
                error.to_string(),
            )
        })?;
        let SocketAddr::V4(endpoint) = local else {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener must bind an IPv4 address",
            ));
        };
        if endpoint.ip().is_unspecified() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener may not bind a wildcard address",
            ));
        }
        if endpoint != observed_endpoint {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener address differs from observed owned endpoint",
            ));
        }
        listener.set_nonblocking(true).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "cannot make NAP control listener nonblocking",
                error.to_string(),
            )
        })?;
        Ok(Self {
            network,
            endpoint,
            listener: Some(listener),
        })
    }

    /// Poll without blocking the daemon. No matching fresh ownership evidence
    /// means the socket is dropped before any queued connection is accepted.
    /// A returned stream is unauthenticated and must never imply membership.
    pub fn accept_if_current(
        &mut self,
        observed_network: Option<NetworkId>,
        observed_endpoint: Option<SocketAddrV4>,
    ) -> Result<Option<TcpStream>, CoreError> {
        if self.listener.is_none() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener has already been retired",
            ));
        }
        if observed_network != Some(self.network) || observed_endpoint != Some(self.endpoint) {
            self.listener.take();
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener lost its observed network ownership",
            ));
        }
        let listener = self.listener.as_ref().expect("checked live listener");
        match listener.accept() {
            Ok((socket, _source)) => {
                // Never use a peer's source IP as an identity or authorization.
                socket.set_nonblocking(false).map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::PanFailure,
                        "cannot prepare untrusted control TCP socket",
                        error.to_string(),
                    )
                })?;
                Ok(Some(socket))
            }
            Err(error) if error.kind() == IoErrorKind::WouldBlock => Ok(None),
            Err(error) => {
                self.listener.take();
                Err(CoreError::with_diagnostic(
                    ErrorKind::PanFailure,
                    "NAP control listener accept failed",
                    error.to_string(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::IpPrefix;
    use std::io::{Read, Write};

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn address(owner: NetworkId) -> InterfaceAddress {
        let config = DaemonConfig::default();
        let plan = Ipv4StarAddressPlan::for_network(owner, config.ipv4_address_pool).unwrap();
        InterfaceAddress {
            owner,
            interface: NetworkInterfaceHandle::new(format!("brb-{}", &owner.to_string()[..8]))
                .unwrap(),
            prefix: plan.host,
        }
    }

    #[test]
    fn selected_nap_host_address_is_exact_and_never_wildcard() {
        let owner = network(7);
        let owned = address(owner);
        let result = owned_nap_control_address(
            owner,
            &DaemonConfig::default(),
            std::slice::from_ref(&owned),
        )
        .unwrap();
        assert_eq!(result.port(), CONTROL_TCP_PORT);
        assert_eq!(IpAddr::V4(*result.ip()), owned.prefix.address);
        assert!(!result.ip().is_unspecified());
        // An identical duplicate backend observation is harmless.
        assert_eq!(
            owned_nap_control_address(owner, &DaemonConfig::default(), &[owned.clone(), owned])
                .unwrap(),
            result
        );
    }

    #[test]
    fn missing_foreign_and_wrong_interface_addresses_cannot_bind() {
        let owner = network(7);
        let config = DaemonConfig::default();
        assert_eq!(
            owned_nap_control_address(owner, &config, &[])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        let mut foreign = address(owner);
        foreign.owner = network(9);
        assert_eq!(
            owned_nap_control_address(owner, &config, &[foreign])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
        let mut wrong_iface = address(owner);
        wrong_iface.interface = NetworkInterfaceHandle::new("eth0").unwrap();
        assert_eq!(
            owned_nap_control_address(owner, &config, &[wrong_iface])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
        assert_eq!(
            owned_nap_control_address(network(0), &config, &[])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn conflicting_owned_bridge_address_fails_closed() {
        let owner = network(7);
        let good = address(owner);
        let mut conflicting = good.clone();
        conflicting.prefix =
            IpPrefix::new("10.201.22.1".parse().unwrap(), good.prefix.prefix_len).unwrap();
        assert_eq!(
            owned_nap_control_address(owner, &DaemonConfig::default(), &[good, conflicting])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn foreign_network_on_owned_bridge_cannot_bind_even_with_distinct_ip() {
        let owner = network(7);
        let owned = address(owner);
        let mut foreign = address(network(9));
        assert_ne!(owned.prefix.address, foreign.prefix.address);
        foreign.interface = owned.interface.clone();
        assert_eq!(
            owned_nap_control_address(owner, &DaemonConfig::default(), &[owned, foreign])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
    }

    #[test]
    fn binding_does_not_try_wildcard_when_no_host_is_observed() {
        assert_eq!(
            bind_owned_nap_control_listener(network(7), &DaemonConfig::default(), &[])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn observed_listener_accepts_only_after_matching_local_ownership_snapshot() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
        assert!(
            owner
                .accept_if_current(Some(network(7)), Some(addr))
                .unwrap()
                .is_none()
        );
        let mut remote = TcpStream::connect(addr).unwrap();
        remote.write_all(b"x").unwrap();
        let mut inbound = owner
            .accept_if_current(Some(network(7)), Some(addr))
            .unwrap()
            .unwrap();
        let mut byte = [0; 1];
        inbound.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"x");
        // A socket being accepted is not authenticated peer evidence.
    }

    #[test]
    fn observed_listener_retires_on_missing_wrong_network_or_changed_endpoint() {
        for (network_observation, endpoint_change) in [
            (None, false),
            (Some(network(9)), false),
            (Some(network(7)), true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = match listener.local_addr().unwrap() {
                SocketAddr::V4(addr) => addr,
                SocketAddr::V6(_) => panic!("test listener must be IPv4"),
            };
            let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
            let endpoint = if endpoint_change {
                Some(SocketAddrV4::new(*addr.ip(), addr.port().wrapping_add(1)))
            } else {
                Some(addr)
            };
            assert_eq!(
                owner
                    .accept_if_current(network_observation, endpoint)
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidState
            );
            assert_eq!(
                owner
                    .accept_if_current(Some(network(7)), Some(addr))
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidState
            );
            assert!(TcpStream::connect(addr).is_err());
        }
    }

    #[test]
    fn observed_listener_rejects_socket_bound_to_a_different_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let actual = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let unexpected = SocketAddrV4::new(*actual.ip(), 46071);
        let observed = if unexpected == actual {
            SocketAddrV4::new(*actual.ip(), 46072)
        } else {
            unexpected
        };
        assert_eq!(
            ObservedControlListener::new(network(7), observed, listener)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::InvalidState
        );
        assert!(TcpStream::connect(actual).is_err());
    }

    #[test]
    fn observed_listener_rejects_queued_socket_after_local_ownership_loss() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let actual = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), actual, listener).unwrap();
        let _queued = TcpStream::connect(actual).unwrap();
        assert_eq!(
            owner
                .accept_if_current(None, Some(actual))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(
            owner
                .accept_if_current(Some(network(7)), Some(actual))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn observed_listener_rejects_zero_network_and_wildcard_binding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        assert_eq!(
            ObservedControlListener::new(network(0), addr, listener)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::InvalidInput
        );
        let wildcard = TcpListener::bind("0.0.0.0:0").unwrap();
        let wildcard_addr = match wildcard.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        assert_eq!(
            ObservedControlListener::new(network(7), wildcard_addr, wildcard)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::InvalidState
        );
    }
}

//! Host-address binding boundary for the P7 TCP-over-PAN listener.
//!
//! This checks the deterministic NAP IPv4 endpoint against independently
//! observed NetworkManager-owned addresses. It does not authenticate a peer,
//! begin accepting sockets, activate JoinNetwork, or prove Bluetooth traffic.

use std::net::{IpAddr, SocketAddrV4, TcpListener};

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

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::IpPrefix;

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
    fn binding_does_not_try_wildcard_when_no_host_is_observed() {
        assert_eq!(
            bind_owned_nap_control_listener(network(7), &DaemonConfig::default(), &[])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }
}

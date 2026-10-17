//! P8-005: project logical route intents into Linux IPv4 route descriptions.
//! This does not authenticate peers, mutate routes, or confer kernel ownership.
//! Callers must supply bindings from verified live PAN and membership state.
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};

use blueroute_core::{
    CoreError, ErrorKind, IpPrefix, NetworkId, NodeId, Route, RouteDestination, RouteOwner,
};

use crate::{LinuxRoute, NetworkInterfaceHandle};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteDestinationBinding {
    pub network: NetworkId,
    pub destination: RouteDestination,
    pub prefix: IpPrefix,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteNextHopBinding {
    pub network: NetworkId,
    pub node: NodeId,
    pub address: Ipv4Addr,
    pub interface: NetworkInterfaceHandle,
}

fn invalid(reason: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidInput, reason)
}

fn nonzero(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| *byte != 0)
}

pub(super) fn private_prefix(prefix: IpPrefix) -> Result<(), CoreError> {
    let IpAddr::V4(address) = prefix.address else {
        return Err(invalid("IPv6 routing is not enabled"));
    };
    let len = prefix.prefix_len;
    if len == 0 || len > 32 {
        return Err(invalid("invalid or default route prefix"));
    }
    let bits = u32::from(address);
    let mask = u32::MAX.checked_shl(u32::from(32 - len)).unwrap_or(0);
    if bits & mask != bits {
        return Err(invalid("noncanonical route prefix"));
    }
    let rfc1918 = (bits & 0xff00_0000 == 0x0a00_0000 && len >= 8)
        || (bits & 0xfff0_0000 == 0xac10_0000 && len >= 12)
        || (bits & 0xffff_0000 == 0xc0a8_0000 && len >= 16);
    if !rfc1918 {
        return Err(invalid("route is not wholly within RFC1918 space"));
    }
    Ok(())
}

fn destination_valid(destination: &RouteDestination) -> Result<(), CoreError> {
    match destination {
        RouteDestination::Internet => Err(invalid("default routes are not supported")),
        RouteDestination::Node(node) if !nonzero(node.as_bytes()) => {
            Err(invalid("zero destination node"))
        }
        RouteDestination::Segment(segment) if !nonzero(segment.as_bytes()) => {
            Err(invalid("zero destination segment"))
        }
        RouteDestination::Prefix(prefix) => private_prefix(*prefix),
        _ => Ok(()),
    }
}

/// Produce deterministic, ownership-scoped route descriptions without writes.
/// The privileged backend must independently reobserve and atomically check
/// actual kernel ownership before applying any resulting route.
pub fn project_ipv4_route_intents(
    network: NetworkId,
    routes: &[Route],
    destinations: &[RouteDestinationBinding],
    next_hops: &[RouteNextHopBinding],
) -> Result<Vec<LinuxRoute>, CoreError> {
    if !nonzero(network.as_bytes()) {
        return Err(invalid("zero network identity"));
    }
    let mut destination_map = BTreeMap::new();
    let mut destination_prefixes = BTreeSet::new();
    for binding in destinations {
        if binding.network != network {
            return Err(invalid("foreign destination binding"));
        }
        destination_valid(&binding.destination)?;
        private_prefix(binding.prefix)?;
        if matches!(binding.destination, RouteDestination::Node(_))
            && binding.prefix.prefix_len != 32
        {
            return Err(invalid("node route requires an IPv4 host prefix"));
        }
        if let RouteDestination::Prefix(prefix) = binding.destination
            && prefix != binding.prefix
        {
            return Err(invalid("prefix binding mismatch"));
        }
        // Unused bindings are still untrusted: two logical destinations
        // may not claim the same kernel prefix, even if only one route is
        // currently requested.
        if !destination_prefixes.insert(binding.prefix) {
            return Err(invalid("duplicate destination IPv4 prefix binding"));
        }
        if destination_map
            .insert(binding.destination.clone(), binding.prefix)
            .is_some()
        {
            return Err(invalid("duplicate destination binding"));
        }
    }
    let mut hop_map = BTreeMap::new();
    let mut hop_addresses = BTreeSet::new();
    for binding in next_hops {
        if binding.network != network || !nonzero(binding.node.as_bytes()) {
            return Err(invalid("foreign or zero next-hop binding"));
        }
        private_prefix(IpPrefix::new(IpAddr::V4(binding.address), 32)?)?;
        // Duplicate address claims by distinct peers are ambiguous,
        // including across interfaces: the trusted runtime must resolve
        // the identity/address mapping before accepting route intents.
        if !hop_addresses.insert(binding.address) {
            return Err(invalid("duplicate next-hop IPv4 address binding"));
        }
        if hop_map.insert(binding.node, binding).is_some() {
            return Err(invalid("duplicate next-hop binding"));
        }
    }
    let mut output = BTreeMap::new();
    for route in routes {
        if route.owner != RouteOwner::BlueRouteNetwork(network) {
            return Err(invalid("foreign route owner"));
        }
        destination_valid(&route.destination)?;
        if route.cost == 0 || !nonzero(route.next_hop.as_bytes()) {
            return Err(invalid("invalid route cost or next hop"));
        }
        let prefix = *destination_map
            .get(&route.destination)
            .ok_or_else(|| invalid("unresolved destination"))?;
        let hop = hop_map
            .get(&route.next_hop)
            .ok_or_else(|| invalid("unobserved next hop"))?;
        let via = if prefix.prefix_len == 32 && prefix.address == IpAddr::V4(hop.address) {
            // A host route is on-link only when the destination is the
            // authenticated first hop itself. A different logical node
            // claiming that address must not be treated as directly reached.
            if route.destination != RouteDestination::Node(route.next_hop) {
                return Err(invalid("destination aliases another next-hop identity"));
            }
            None
        } else {
            Some(IpAddr::V4(hop.address))
        };
        let projected = LinuxRoute {
            destination: prefix,
            via,
            interface: hop.interface.clone(),
            metric: route.cost,
            owner: network,
        };
        if output.insert(prefix, projected).is_some() {
            return Err(invalid("multiple logical routes alias one kernel prefix"));
        }
    }
    Ok(output.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(n: u8) -> NetworkId {
        NetworkId::from_bytes([n; 16])
    }

    fn node(n: u8) -> NodeId {
        NodeId::from_bytes([n; 16])
    }

    fn prefix(address: [u8; 4], len: u8) -> IpPrefix {
        IpPrefix::new(IpAddr::V4(Ipv4Addr::from(address)), len).unwrap()
    }

    fn route(n: u8) -> Route {
        Route {
            destination: RouteDestination::Node(node(n)),
            next_hop: node(2),
            cost: 5,
            owner: RouteOwner::BlueRouteNetwork(network(1)),
        }
    }

    fn binding(n: u8, address: [u8; 4]) -> RouteDestinationBinding {
        RouteDestinationBinding {
            network: network(1),
            destination: RouteDestination::Node(node(n)),
            prefix: prefix(address, 32),
        }
    }

    fn hop() -> RouteNextHopBinding {
        RouteNextHopBinding {
            network: network(1),
            node: node(2),
            address: Ipv4Addr::new(10, 20, 0, 2),
            interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
        }
    }

    #[test]
    fn direct_and_routed_nodes_have_deterministic_ipv4_projection() {
        let result = project_ipv4_route_intents(
            network(1),
            &[route(3), route(2)],
            &[binding(3, [10, 20, 0, 3]), binding(2, [10, 20, 0, 2])],
            &[hop()],
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].via, None);
        assert_eq!(result[1].via, Some(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 2))));
        assert!(result.iter().all(|route| route.owner == network(1)));
    }

    #[test]
    fn unrelated_node_cannot_become_direct_by_aliasing_next_hop_address() {
        let aliased = binding(3, [10, 20, 0, 2]);
        assert!(project_ipv4_route_intents(network(1), &[route(3)], &[aliased], &[hop()]).is_err());
        let direct = project_ipv4_route_intents(
            network(1),
            &[route(2)],
            &[binding(2, [10, 20, 0, 2])],
            &[hop()],
        )
        .unwrap();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].via, None);
    }

    #[test]
    fn unknown_or_foreign_bindings_do_not_project_routes() {
        let destination = binding(3, [10, 20, 0, 3]);
        assert!(project_ipv4_route_intents(network(1), &[route(3)], &[], &[hop()]).is_err());
        assert!(
            project_ipv4_route_intents(
                network(1),
                &[route(3)],
                std::slice::from_ref(&destination),
                &[]
            )
            .is_err()
        );
        let mut foreign = destination.clone();
        foreign.network = network(2);
        assert!(project_ipv4_route_intents(network(1), &[route(3)], &[foreign], &[hop()]).is_err());
        let mut foreign_hop = hop();
        foreign_hop.network = network(2);
        assert!(
            project_ipv4_route_intents(network(1), &[route(3)], &[destination], &[foreign_hop])
                .is_err()
        );
    }

    #[test]
    fn different_destinations_cannot_claim_the_same_kernel_host_prefix() {
        let primary = binding(3, [10, 20, 0, 3]);
        let alias = binding(4, [10, 20, 0, 3]);
        let result =
            project_ipv4_route_intents(network(1), &[route(3)], &[primary, alias], &[hop()]);
        assert!(result.is_err());
    }

    #[test]
    fn two_peer_identities_cannot_share_one_next_hop_ipv4_address() {
        let mut alias = hop();
        alias.node = node(4);
        let destination = binding(3, [10, 20, 0, 3]);
        let result = project_ipv4_route_intents(
            network(1),
            &[route(3)],
            std::slice::from_ref(&destination),
            &[hop(), alias.clone()],
        );
        assert!(result.is_err());

        alias.address = Ipv4Addr::new(10, 21, 0, 4);
        let valid =
            project_ipv4_route_intents(network(1), &[route(3)], &[destination], &[hop(), alias]);
        assert!(valid.is_ok());
    }

    #[test]
    fn rejects_public_noncanonical_and_overbroad_prefixes() {
        for unsafe_prefix in [
            prefix([10, 0, 0, 0], 7),
            prefix([10, 20, 0, 77], 24),
            prefix([8, 8, 8, 8], 32),
            prefix([0, 0, 0, 0], 0),
            IpPrefix::new(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), 128).unwrap(),
        ] {
            let mut destination = binding(3, [10, 20, 0, 3]);
            destination.prefix = unsafe_prefix;
            assert!(
                project_ipv4_route_intents(network(1), &[route(3)], &[destination], &[hop()])
                    .is_err()
            );
        }
        let mut bad_hop = hop();
        bad_hop.address = Ipv4Addr::new(8, 8, 8, 8);
        assert!(
            project_ipv4_route_intents(
                network(1),
                &[route(3)],
                &[binding(3, [10, 20, 0, 3])],
                &[bad_hop]
            )
            .is_err()
        );
    }

    #[test]
    fn node_binding_cannot_claim_an_entire_private_subnet() {
        let mut destination = binding(3, [10, 20, 0, 3]);
        destination.prefix = prefix([10, 20, 0, 0], 24);
        assert!(
            project_ipv4_route_intents(network(1), &[route(3)], &[destination], &[hop()]).is_err()
        );
    }

    #[test]
    fn duplicate_kernel_destinations_and_default_route_fail_closed() {
        assert!(
            project_ipv4_route_intents(
                network(1),
                &[route(2), route(3)],
                &[binding(2, [10, 20, 0, 3]), binding(3, [10, 20, 0, 3])],
                &[hop()],
            )
            .is_err()
        );
        let mut internet = route(3);
        internet.destination = RouteDestination::Internet;
        assert!(project_ipv4_route_intents(network(1), &[internet], &[], &[hop()]).is_err());
        assert!(project_ipv4_route_intents(network(0), &[], &[], &[]).is_err());
    }
}

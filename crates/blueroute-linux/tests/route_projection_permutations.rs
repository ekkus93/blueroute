//! P8-005 deterministic route projection integration qualification.
//! These are software-only tests: bindings are caller-supplied, not authenticated.
use std::net::{IpAddr, Ipv4Addr};
use blueroute_core::{IpPrefix, NetworkId, NodeId, Route, RouteDestination, RouteOwner};
use blueroute_linux::{NetworkInterfaceHandle, RouteDestinationBinding, RouteNextHopBinding, project_ipv4_route_intents};

fn network() -> NetworkId { NetworkId::from_bytes([1; 16]) }
fn node(n: u8) -> NodeId { NodeId::from_bytes([n; 16]) }
fn destination(n: u8) -> RouteDestinationBinding {
    RouteDestinationBinding {
        network: network(),
        destination: RouteDestination::Node(node(n)),
        prefix: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, n)), 32).unwrap(),
    }
}
fn hop(n: u8) -> RouteNextHopBinding {
    RouteNextHopBinding {
        network: network(),
        node: node(n),
        address: Ipv4Addr::new(10, 20, 0, n),
        interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
    }
}
fn route(destination: u8, next_hop: u8) -> Route {
    Route {
        destination: RouteDestination::Node(node(destination)),
        next_hop: node(next_hop),
        cost: 5,
        owner: RouteOwner::BlueRouteNetwork(network()),
    }
}

#[test]
fn permutation_of_untrusted_binding_input_cannot_change_route_output() {
    let routes = [route(3, 2), route(2, 2)];
    let destinations = [destination(2), destination(3)];
    let original = project_ipv4_route_intents(network(), &routes, &destinations, &[hop(2)]).unwrap();
    let reversed = project_ipv4_route_intents(
        network(),
        &[routes[1].clone(), routes[0].clone()],
        &[destinations[1].clone(), destinations[0].clone()],
        &[hop(2)],
    ).unwrap();
    assert_eq!(original, reversed);
    assert_eq!(original.len(), 2);
}

#[test]
fn unused_duplicate_peer_address_binding_is_rejected() {
    let mut alias = hop(4);
    alias.address = Ipv4Addr::new(10, 20, 0, 2);
    assert!(project_ipv4_route_intents(
        network(), &[route(3, 2)], &[destination(3)], &[hop(2), alias]
    ).is_err());
}

#[test]
#[ignore = "P8-005: cross-peer destination alias guard still needs implementation"]
fn destination_cannot_alias_another_observed_next_hop_even_if_unused() {
    let mut aliased = destination(3);
    aliased.prefix = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 4)), 32).unwrap();
    assert!(project_ipv4_route_intents(
        network(), &[route(5, 2)],
        &[destination(5), aliased], &[hop(2), hop(4)]
    ).is_err());
}

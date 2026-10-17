//! P8-005 deterministic route projection integration qualification.
//! These are software-only tests: bindings are caller-supplied, not authenticated.
use blueroute_core::{IpPrefix, NetworkId, NodeId, Route, RouteDestination, RouteOwner};
use blueroute_linux::{
    NetworkInterfaceHandle, RouteDestinationBinding, RouteNextHopBinding,
    project_ipv4_route_intents,
};
use std::net::{IpAddr, Ipv4Addr};

fn network() -> NetworkId {
    NetworkId::from_bytes([1; 16])
}
fn node(n: u8) -> NodeId {
    NodeId::from_bytes([n; 16])
}
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
    let original =
        project_ipv4_route_intents(network(), &routes, &destinations, &[hop(2)]).unwrap();
    let reversed = project_ipv4_route_intents(
        network(),
        &[routes[1].clone(), routes[0].clone()],
        &[destinations[1].clone(), destinations[0].clone()],
        &[hop(2)],
    )
    .unwrap();
    assert_eq!(original, reversed);
    assert_eq!(original.len(), 2);
}

#[test]
fn unused_duplicate_peer_address_binding_is_rejected() {
    let mut alias = hop(4);
    alias.address = Ipv4Addr::new(10, 20, 0, 2);
    assert!(
        project_ipv4_route_intents(
            network(),
            &[route(3, 2)],
            &[destination(3)],
            &[hop(2), alias]
        )
        .is_err()
    );
}

#[test]
fn destination_cannot_alias_another_observed_next_hop_even_if_unused() {
    let mut aliased = destination(3);
    aliased.prefix = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 4)), 32).unwrap();
    assert!(
        project_ipv4_route_intents(
            network(),
            &[route(5, 2)],
            &[destination(5), aliased],
            &[hop(2), hop(4)]
        )
        .is_err()
    );
}

#[test]
fn cross_peer_alias_rejection_is_independent_of_input_order_and_route_selection() {
    let mut aliased = destination(3);
    aliased.prefix = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 4)), 32).unwrap();
    for destinations in [
        [destination(5), aliased.clone()],
        [aliased.clone(), destination(5)],
    ] {
        for next_hops in [[hop(2), hop(4)], [hop(4), hop(2)]] {
            // A route via peer 2 cannot legitimize node 3 claiming peer 4's address.
            assert!(
                project_ipv4_route_intents(network(), &[route(5, 2)], &destinations, &next_hops)
                    .is_err()
            );
        }
    }
}

#[test]
fn distinct_peer_addresses_allow_direct_and_transit_paths() {
    let projected = project_ipv4_route_intents(
        network(),
        &[route(4, 4), route(5, 4), route(6, 2)],
        &[destination(4), destination(5), destination(6)],
        &[hop(2), hop(4)],
    )
    .unwrap();
    assert_eq!(projected.len(), 3);
    assert_eq!(projected[0].destination, destination(4).prefix);
    assert_eq!(projected[0].via, None);
    assert_eq!(
        projected[1].via,
        Some(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 4)))
    );
    assert_eq!(
        projected[2].via,
        Some(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 2)))
    );
}

#[test]
fn unused_observed_peer_binding_must_match_observed_next_hop_address() {
    // Peer 4 is observed at .4; the unneeded destination binding claims .99.
    // Neither its position nor which next hop is selected can legalize it.
    let mut alternate = destination(4);
    alternate.prefix = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 99)), 32).unwrap();
    for destinations in [
        [destination(5), alternate.clone()],
        [alternate.clone(), destination(5)],
    ] {
        for next_hops in [[hop(2), hop(4)], [hop(4), hop(2)]] {
            assert!(
                project_ipv4_route_intents(network(), &[route(5, 2)], &destinations, &next_hops)
                    .is_err()
            );
        }
    }
    // A direct peer and a distinct routed destination remain valid.
    let valid = project_ipv4_route_intents(
        network(),
        &[route(5, 2)],
        &[destination(5), destination(4)],
        &[hop(2), hop(4)],
    )
    .unwrap();
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].via, Some(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 2))));
}

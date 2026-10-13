//! Deterministic software-only route computation from active PAN links.
//! This module neither installs kernel routes nor accepts remote route
//! advertisements. P7 authenticated authorization and P8-005 distribution
//! must gate every privileged route mutation.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use crate::{
    CoreError, ErrorKind, LinkHealth, NetworkId, NodeId, Route, RouteDestination,
    RouteOwner, TopologyGraph,
};

/// Degraded links are less preferred without discarding them entirely.
pub const DEGRADED_LINK_COST_PENALTY: u64 = 100;

/// Compute per-source node routes, excluding the local node. Every transit
/// node MUST explicitly advertise routing capability; unknown is not true.
/// A non-routing endpoint may still be directly reachable as a destination.
///
/// The weighted shortest path is stable across insertion orders. Ties favor
/// fewer hops and then the lowest first-hop NodeId. The caller must ensure
/// that graph links represent authenticated, active local observations.
pub fn desired_node_routes(
    graph: &TopologyGraph,
    source: NodeId,
    network: NetworkId,
) -> Result<Vec<Route>, CoreError> {
    if graph.node(&source).is_none() {
        return Err(CoreError::new(ErrorKind::InvalidInput, "source is not in the topology"));
    }
    // Each node's tuple is (total cost, hop count, first hop).
    let mut best = BTreeMap::<NodeId, (u64, usize, NodeId)>::new();
    let mut queue = BinaryHeap::<Reverse<(u64, usize, NodeId, NodeId)>>::new();
    best.insert(source, (0, 0, source));
    queue.push(Reverse((0, 0, source, source)));
    while let Some(Reverse((cost, hops, first_hop, current))) = queue.pop() {
        if best.get(&current) != Some(&(cost, hops, first_hop)) {
            continue;
        }
        if current != source
            && graph.node(&current).and_then(|caps| caps.can_route()) != Some(true)
        {
            continue;
        }
        for (_, link) in graph.links() {
            if !link.is_forwarding_candidate() {
                continue;
            }
            let Some(next) = link.other_endpoint(current) else {
                continue;
            };
            let edge = u64::from(link.cost.max(1))
                + if link.health == LinkHealth::Degraded {
                    DEGRADED_LINK_COST_PENALTY
                } else {
                    0
                };
            let next_cost = cost.checked_add(edge).ok_or_else(|| {
                CoreError::new(ErrorKind::TopologyFailure, "route cost overflow")
            })?;
            let next_hops = hops + 1;
            let next_first_hop = if current == source { next } else { first_hop };
            let candidate = (next_cost, next_hops, next_first_hop);
            if best.get(&next).is_none_or(|previous| candidate < *previous) {
                best.insert(next, candidate);
                queue.push(Reverse((next_cost, next_hops, next_first_hop, next)));
            }
        }
    }
    best.into_iter()
        .filter(|(node, _)| *node != source)
        .map(|(node, (cost, _, next_hop))| {
            let cost = u32::try_from(cost).map_err(|_| {
                CoreError::new(ErrorKind::TopologyFailure, "route metric exceeds u32")
            })?;
            Ok(Route {
                destination: RouteDestination::Node(node),
                next_hop,
                cost,
                owner: RouteOwner::BlueRouteNetwork(network),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CapabilitySource, LinkHealth, LinkId, LinkState, NodeCapabilities,
        PanLink, PanSegment, SegmentId, Sourced,
    };

    fn node(id: u8) -> NodeId {
        NodeId::from_bytes([id; 16])
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([9; 16])
    }

    fn add_node(graph: &mut TopologyGraph, id: u8, router: Option<bool>) {
        graph.add_node(
            node(id),
            NodeCapabilities {
                routing: router.map(|v| Sourced::new(v, CapabilitySource::Configured)),
                ..NodeCapabilities::default()
            },
        );
        graph.add_segment(PanSegment::new(SegmentId::from_bytes([id; 16]), node(id))).unwrap();
    }

    fn link(
        graph: &mut TopologyGraph,
        id: u8,
        nap: u8,
        panu: u8,
        cost: u32,
        health: LinkHealth,
        state: LinkState,
    ) {
        graph.add_link(PanLink {
            id: LinkId::from_bytes([id; 16]),
            segment_id: SegmentId::from_bytes([nap; 16]),
            nap: node(nap),
            panu: node(panu),
            state,
            health,
            cost,
        }).unwrap();
    }

    fn next_hop_to(routes: &[Route], target: NodeId) -> Option<NodeId> {
        routes.iter().find_map(|r| match r.destination {
            RouteDestination::Node(n) if n == target => Some(r.next_hop),
            _ => None,
        })
    }

    #[test]
    fn line_route_requires_explicit_transit_capability() {
        let mut graph = TopologyGraph::default();
        add_node(&mut graph, 1, None);
        add_node(&mut graph, 2, Some(true));
        add_node(&mut graph, 3, Some(false));
        link(&mut graph, 1, 1, 2, 1, LinkHealth::Healthy, LinkState::Active);
        link(&mut graph, 2, 2, 3, 1, LinkHealth::Healthy, LinkState::Active);
        let routes = desired_node_routes(&graph, node(1), network()).unwrap();
        assert_eq!(next_hop_to(&routes, node(2)), Some(node(2)));
        assert_eq!(next_hop_to(&routes, node(3)), Some(node(2)));
        assert!(routes.iter().all(|r| r.owner == RouteOwner::BlueRouteNetwork(network())));
        assert!(!routes.iter().any(|r| r.destination == RouteDestination::Internet));

        let mut blocked = TopologyGraph::default();
        add_node(&mut blocked, 1, None);
        add_node(&mut blocked, 2, None);
        add_node(&mut blocked, 3, Some(false));
        link(&mut blocked, 1, 1, 2, 1, LinkHealth::Healthy, LinkState::Active);
        link(&mut blocked, 2, 2, 3, 1, LinkHealth::Healthy, LinkState::Active);
        let routes = desired_node_routes(&blocked, node(1), network()).unwrap();
        assert_eq!(next_hop_to(&routes, node(2)), Some(node(2)));
        assert_eq!(next_hop_to(&routes, node(3)), None);
    }

    #[test]
    fn degraded_links_lose_to_stable_healthy_alternative() {
        let mut graph = TopologyGraph::default();
        for id in 1..=4 {
            add_node(&mut graph, id, Some(true));
        }
        link(&mut graph, 1, 1, 2, 1, LinkHealth::Degraded, LinkState::Active);
        link(&mut graph, 2, 2, 4, 1, LinkHealth::Healthy, LinkState::Active);
        link(&mut graph, 3, 1, 3, 2, LinkHealth::Healthy, LinkState::Active);
        link(&mut graph, 4, 3, 4, 2, LinkHealth::Healthy, LinkState::Active);
        let routes = desired_node_routes(&graph, node(1), network()).unwrap();
        assert_eq!(next_hop_to(&routes, node(4)), Some(node(3)));
        assert_eq!(routes.iter().find(|r| r.destination == RouteDestination::Node(node(4))).unwrap().cost, 4);
    }

    #[test]
    fn failed_and_disconnected_links_never_supply_routes() {
        let mut graph = TopologyGraph::default();
        for id in 1..=3 { add_node(&mut graph, id, Some(true)); }
        link(&mut graph, 1, 1, 2, 1, LinkHealth::Failed, LinkState::Active);
        link(&mut graph, 2, 1, 3, 1, LinkHealth::Healthy, LinkState::Disconnected);
        assert!(desired_node_routes(&graph, node(1), network()).unwrap().is_empty());
    }

    #[test]
    fn unknown_source_or_partition_and_client_only_nodes_fail_closed() {
        let mut graph = TopologyGraph::default();
        add_node(&mut graph, 1, Some(false));
        add_node(&mut graph, 2, Some(false));
        add_node(&mut graph, 3, None);
        assert!(desired_node_routes(&graph, node(9), network()).is_err());
        assert!(desired_node_routes(&graph, node(1), network()).unwrap().is_empty());
        link(&mut graph, 1, 1, 2, 1, LinkHealth::Healthy, LinkState::Active);
        let routes = desired_node_routes(&graph, node(1), network()).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(next_hop_to(&routes, node(3)), None);
    }

    #[test]
    fn equivalent_route_ties_are_deterministic() {
        let mut graph = TopologyGraph::default();
        for id in 1..=4 { add_node(&mut graph, id, Some(true)); }
        link(&mut graph, 4, 1, 3, 1, LinkHealth::Healthy, LinkState::Active);
        link(&mut graph, 3, 3, 4, 1, LinkHealth::Healthy, LinkState::Active);
        link(&mut graph, 2, 1, 2, 1, LinkHealth::Healthy, LinkState::Active);
        link(&mut graph, 1, 2, 4, 1, LinkHealth::Healthy, LinkState::Active);
        let routes = desired_node_routes(&graph, node(1), network()).unwrap();
        assert_eq!(next_hop_to(&routes, node(4)), Some(node(2)));
    }
}

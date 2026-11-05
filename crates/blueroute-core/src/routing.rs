//! Route computation v1 (P8-004).
//!
//! Pure and deterministic. Works on PAN *segments* (broadcast domains), not on individual
//! nodes: a node transits traffic between two segments only when it is attached to both on
//! active links and its `routing` capability is known to be `true`. Same-segment peers need no
//! route (connected subnet), and a NAP that merely bridges its own segment is not a router.
//! Equal-cost alternatives are resolved by preferring the previous next hop (stability), then
//! the lowest `NodeId` (determinism).

use std::collections::{BTreeMap, BTreeSet};

use crate::{NetworkId, NodeId, Route, RouteDestination, RouteOwner, SegmentId, TopologyGraph};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComputedRoutes {
    /// Desired routes for `local`, sorted by destination.
    pub routes: Vec<Route>,
    /// Segments that exist in the topology but cannot be reached from `local`.
    pub unreachable: BTreeSet<SegmentId>,
}

type Attachments = BTreeMap<SegmentId, BTreeMap<NodeId, u32>>;

/// Which nodes are attached to which segments over active links, with the cheapest attach cost.
fn attachments(graph: &TopologyGraph) -> Attachments {
    let mut map: Attachments = BTreeMap::new();
    for (id, segment) in graph.segments() {
        map.entry(*id).or_default().insert(segment.nap, 0);
    }
    for (_, link) in graph.links() {
        if !link.is_forwarding_candidate() {
            continue;
        }
        let slot = map
            .entry(link.segment_id)
            .or_default()
            .entry(link.panu)
            .or_insert(link.cost);
        *slot = (*slot).min(link.cost);
    }
    map
}

pub fn compute_routes(
    graph: &TopologyGraph,
    local: NodeId,
    network: NetworkId,
    previous: &[Route],
) -> ComputedRoutes {
    let attached = attachments(graph);
    let routers: BTreeSet<NodeId> = graph
        .nodes()
        .filter(|(_, caps)| caps.can_route() == Some(true))
        .map(|(id, _)| *id)
        .collect();

    // node -> segments it is attached to
    let mut by_node: BTreeMap<NodeId, BTreeMap<SegmentId, u32>> = BTreeMap::new();
    for (segment, nodes) in &attached {
        for (node, cost) in nodes {
            by_node.entry(*node).or_default().insert(*segment, *cost);
        }
    }

    let local_segments: BTreeSet<SegmentId> = by_node
        .get(&local)
        .map(|m| m.keys().copied().collect())
        .unwrap_or_default();

    let mut dist: BTreeMap<SegmentId, u64> = BTreeMap::new();
    let mut first_hops: BTreeMap<SegmentId, BTreeSet<NodeId>> = BTreeMap::new();
    let mut queue: BTreeSet<(u64, SegmentId)> = BTreeSet::new();
    for segment in &local_segments {
        dist.insert(*segment, 0);
        queue.insert((0, *segment));
    }

    while let Some((distance, current)) = queue.pop_first() {
        if dist.get(&current).is_some_and(|d| *d < distance) {
            continue;
        }
        let Some(nodes) = attached.get(&current) else {
            continue;
        };
        for (router, enter_cost) in nodes {
            if *router == local || !routers.contains(router) {
                continue;
            }
            let hops: BTreeSet<NodeId> = if local_segments.contains(&current) {
                BTreeSet::from([*router])
            } else {
                first_hops.get(&current).cloned().unwrap_or_default()
            };
            let Some(router_segments) = by_node.get(router) else {
                continue;
            };
            for (next, leave_cost) in router_segments {
                if *next == current {
                    continue;
                }
                let candidate = distance + u64::from(*enter_cost) + u64::from(*leave_cost);
                match dist.get(next).copied() {
                    Some(existing) if existing < candidate => {}
                    Some(existing) if existing == candidate => {
                        first_hops
                            .entry(*next)
                            .or_default()
                            .extend(hops.iter().copied());
                    }
                    _ => {
                        if let Some(old) = dist.insert(*next, candidate) {
                            queue.remove(&(old, *next));
                        }
                        first_hops.insert(*next, hops.clone());
                        queue.insert((candidate, *next));
                    }
                }
            }
        }
    }

    let mut result = ComputedRoutes::default();
    for (segment, _) in graph.segments() {
        if local_segments.contains(segment) {
            continue;
        }
        match (dist.get(segment), first_hops.get(segment)) {
            (Some(cost), Some(hops)) if !hops.is_empty() => {
                let destination = RouteDestination::Segment(*segment);
                let kept = previous
                    .iter()
                    .find(|r| r.destination == destination)
                    .map(|r| r.next_hop)
                    .filter(|hop| hops.contains(hop));
                let next_hop = kept.or_else(|| hops.iter().next().copied()).unwrap();
                result.routes.push(Route {
                    destination,
                    next_hop,
                    cost: u32::try_from(*cost).unwrap_or(u32::MAX),
                    owner: RouteOwner::BlueRouteNetwork(network),
                });
            }
            _ => {
                result.unreachable.insert(*segment);
            }
        }
    }
    result.routes.sort();
    result
}

/// Most routes accepted in one assignment (bounds memory for hostile input).
pub const MAX_ASSIGNED_ROUTES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteRejection {
    /// Only the network authority may assign routes.
    SenderNotAuthority,
    TooManyRoutes,
    ForeignOwner,
    /// v1 assigns segment routes only; other destination kinds are refused.
    UnsupportedDestination,
    UnknownSegment(SegmentId),
    NextHopNotMember(NodeId),
    NextHopIsLocal,
    DuplicateDestination,
}

/// Validate a route assignment received over the control session. Fails closed: a single bad
/// route rejects the whole assignment so a compromised or buggy sender cannot partially apply.
pub fn validate_route_assignment(
    routes: &[Route],
    network: NetworkId,
    sender: NodeId,
    authority: NodeId,
    local: NodeId,
    members: &BTreeSet<NodeId>,
    known_segments: &BTreeSet<SegmentId>,
) -> Result<(), RouteRejection> {
    if sender != authority {
        return Err(RouteRejection::SenderNotAuthority);
    }
    if routes.len() > MAX_ASSIGNED_ROUTES {
        return Err(RouteRejection::TooManyRoutes);
    }
    let mut seen = BTreeSet::new();
    for route in routes {
        if route.owner != RouteOwner::BlueRouteNetwork(network) {
            return Err(RouteRejection::ForeignOwner);
        }
        let RouteDestination::Segment(segment) = &route.destination else {
            return Err(RouteRejection::UnsupportedDestination);
        };
        if !known_segments.contains(segment) {
            return Err(RouteRejection::UnknownSegment(*segment));
        }
        if route.next_hop == local {
            return Err(RouteRejection::NextHopIsLocal);
        }
        if !members.contains(&route.next_hop) {
            return Err(RouteRejection::NextHopNotMember(route.next_hop));
        }
        if !seen.insert(route.destination.clone()) {
            return Err(RouteRejection::DuplicateDestination);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CapabilitySource, LinkHealth, LinkId, LinkState, NodeCapabilities, PanLink, PanSegment,
        Sourced,
    };

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn s(b: u8) -> SegmentId {
        SegmentId::from_bytes([b; 16])
    }

    fn caps(routing: Option<bool>) -> NodeCapabilities {
        NodeCapabilities {
            routing: routing.map(|v| Sourced::new(v, CapabilitySource::Configured)),
            ..NodeCapabilities::default()
        }
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([5; 16])
    }

    /// `nodes`: (id, routing). `segs`: (segment, nap). `links`: (link id, segment, panu, cost).
    fn graph(
        nodes: &[(u8, Option<bool>)],
        segs: &[(u8, u8)],
        links: &[(u8, u8, u8, u32)],
    ) -> TopologyGraph {
        let mut g = TopologyGraph::default();
        for (id, routing) in nodes {
            g.add_node(n(*id), caps(*routing));
        }
        for (seg, nap) in segs {
            g.add_segment(PanSegment::new(s(*seg), n(*nap))).unwrap();
        }
        for (id, seg, panu, cost) in links {
            let nap = segs.iter().find(|(x, _)| x == seg).unwrap().1;
            g.add_link(PanLink {
                id: LinkId::from_bytes([*id; 16]),
                segment_id: s(*seg),
                nap: n(nap),
                panu: n(*panu),
                state: LinkState::Active,
                health: LinkHealth::Healthy,
                cost: *cost,
            })
            .unwrap();
        }
        g
    }

    fn hop(c: &ComputedRoutes, seg: u8) -> Option<NodeId> {
        c.routes
            .iter()
            .find(|r| r.destination == RouteDestination::Segment(s(seg)))
            .map(|r| r.next_hop)
    }

    #[test]
    fn line_topology_routes_through_the_middle_router() {
        // C -(S2, nap B)- B -(S1, nap A)- A.  B is attached to S1 as PANU and S2 as NAP.
        let g = graph(
            &[(1, None), (2, Some(true)), (3, None)],
            &[(1, 1), (2, 2)],
            &[(1, 1, 2, 1), (2, 2, 3, 1)],
        );
        let from_c = compute_routes(&g, n(3), network(), &[]);
        assert_eq!(hop(&from_c, 1), Some(n(2)));
        assert!(from_c.unreachable.is_empty());
        let from_a = compute_routes(&g, n(1), network(), &[]);
        assert_eq!(hop(&from_a, 2), Some(n(2)));
        // B is on both segments: nothing to route.
        assert!(compute_routes(&g, n(2), network(), &[]).routes.is_empty());
    }

    #[test]
    fn same_segment_peers_and_a_plain_bridging_nap_need_no_routes() {
        let g = graph(
            &[(1, None), (2, None), (3, None)],
            &[(1, 1)],
            &[(1, 1, 2, 1), (2, 1, 3, 1)],
        );
        for who in 1..=3 {
            assert_eq!(
                compute_routes(&g, n(who), network(), &[]),
                ComputedRoutes::default()
            );
        }
    }

    #[test]
    fn tree_of_stars_routes_each_leaf_segment_via_the_hub_router() {
        // Hub H(1) nap of S1; routers R2,R3 are PANUs on S1 and NAPs of S2,S3.
        let g = graph(
            &[
                (1, Some(true)),
                (2, Some(true)),
                (3, Some(true)),
                (4, None),
                (5, None),
            ],
            &[(1, 1), (2, 2), (3, 3)],
            &[(1, 1, 2, 1), (2, 1, 3, 1), (3, 2, 4, 1), (4, 3, 5, 1)],
        );
        let from_leaf = compute_routes(&g, n(4), network(), &[]);
        assert_eq!(hop(&from_leaf, 1), Some(n(2)));
        assert_eq!(
            hop(&from_leaf, 3),
            Some(n(2)),
            "S3 is two router hops away via R2"
        );
        assert_eq!(
            from_leaf
                .routes
                .iter()
                .find(|r| r.destination == RouteDestination::Segment(s(3)))
                .unwrap()
                .cost,
            2 // R2 uplink (1) + R3 attach on S1 (1) + R3 NAP on S3 (0)
        );
    }

    #[test]
    fn redundant_paths_are_deterministic_cheapest_and_stable() {
        // Local L(9) on S1; two routers R2,R3 both attached to S1 and S2.
        let nodes = [(9, None), (2, Some(true)), (3, Some(true)), (7, Some(true))];
        let g = graph(
            &nodes,
            &[(1, 7), (2, 7)],
            &[
                (1, 1, 9, 1),
                (2, 1, 2, 1),
                (3, 1, 3, 1),
                (4, 2, 2, 1),
                (5, 2, 3, 1),
            ],
        );
        // Dijkstra sees R7 (the NAP of both) as the cheapest transit (attach cost 0).
        let first = compute_routes(&g, n(9), network(), &[]);
        assert_eq!(hop(&first, 2), Some(n(7)));
        assert_eq!(
            compute_routes(&g, n(9), network(), &[]),
            first,
            "deterministic"
        );

        // With equal costs and no preference the lowest NodeId wins; a previous hop is kept.
        let g = graph(
            &[(9, None), (2, Some(true)), (3, Some(true)), (7, None)],
            &[(1, 7), (2, 7)],
            &[
                (1, 1, 9, 1),
                (2, 1, 2, 1),
                (3, 1, 3, 1),
                (4, 2, 2, 1),
                (5, 2, 3, 1),
            ],
        );
        let fresh = compute_routes(&g, n(9), network(), &[]);
        assert_eq!(hop(&fresh, 2), Some(n(2)));
        let prev = vec![Route {
            destination: RouteDestination::Segment(s(2)),
            next_hop: n(3),
            cost: 99,
            owner: RouteOwner::BlueRouteNetwork(network()),
        }];
        assert_eq!(
            hop(&compute_routes(&g, n(9), network(), &prev), 2),
            Some(n(3))
        );
        // A previous hop that is no longer an optimal path is dropped.
        let prev_gone = vec![Route {
            next_hop: n(8),
            ..prev[0].clone()
        }];
        assert_eq!(
            hop(&compute_routes(&g, n(9), network(), &prev_gone), 2),
            Some(n(2))
        );
    }

    #[test]
    fn partition_after_link_failure_is_reported_unreachable() {
        let mut g = graph(
            &[(1, None), (2, Some(true)), (3, None)],
            &[(1, 1), (2, 2)],
            &[(1, 1, 2, 1), (2, 2, 3, 1)],
        );
        assert_eq!(
            hop(&compute_routes(&g, n(3), network(), &[]), 1),
            Some(n(2))
        );
        g.remove_link(&LinkId::from_bytes([1; 16]));
        let c = compute_routes(&g, n(3), network(), &[]);
        assert!(c.routes.is_empty());
        assert_eq!(c.unreachable, BTreeSet::from([s(1)]));
    }

    #[test]
    fn nodes_without_a_known_routing_capability_never_transit() {
        for routing in [None, Some(false)] {
            let g = graph(
                &[(1, None), (2, routing), (3, None)],
                &[(1, 1), (2, 2)],
                &[(1, 1, 2, 1), (2, 2, 3, 1)],
            );
            let c = compute_routes(&g, n(3), network(), &[]);
            assert!(c.routes.is_empty(), "routing={routing:?}");
            assert_eq!(c.unreachable, BTreeSet::from([s(1)]));
        }
    }

    #[test]
    fn heterogeneous_capabilities_route_around_the_non_router() {
        // Two parallel bridges between S1 and S2: R2 cannot route, R3 can.
        let g = graph(
            &[(1, None), (2, Some(false)), (3, Some(true)), (4, None)],
            &[(1, 1), (2, 4)],
            &[(1, 1, 2, 1), (2, 1, 3, 1), (3, 2, 2, 1), (4, 2, 3, 5)],
        );
        // Local is node 1 (NAP of S1).
        let c = compute_routes(&g, n(1), network(), &[]);
        assert_eq!(hop(&c, 2), Some(n(3)));
    }

    fn route(seg: u8, hop: u8) -> Route {
        Route {
            destination: RouteDestination::Segment(s(seg)),
            next_hop: n(hop),
            cost: 1,
            owner: RouteOwner::BlueRouteNetwork(network()),
        }
    }

    fn validate(routes: &[Route], sender: u8) -> Result<(), RouteRejection> {
        validate_route_assignment(
            routes,
            network(),
            n(sender),
            n(1),
            n(2),
            &BTreeSet::from([n(1), n(2), n(3)]),
            &BTreeSet::from([s(1), s(2)]),
        )
    }

    #[test]
    fn assignments_from_the_authority_for_members_are_accepted() {
        assert_eq!(validate(&[route(1, 3), route(2, 1)], 1), Ok(()));
        assert_eq!(validate(&[], 1), Ok(()));
    }

    #[test]
    fn foreign_non_member_and_malformed_assignments_are_rejected() {
        assert_eq!(
            validate(&[route(1, 3)], 3),
            Err(RouteRejection::SenderNotAuthority)
        );
        let mut foreign = route(1, 3);
        foreign.owner = RouteOwner::BlueRouteNetwork(NetworkId::from_bytes([0; 16]));
        assert_eq!(validate(&[foreign], 1), Err(RouteRejection::ForeignOwner));
        assert_eq!(
            validate(&[route(1, 9)], 1),
            Err(RouteRejection::NextHopNotMember(n(9)))
        );
        assert_eq!(
            validate(&[route(7, 3)], 1),
            Err(RouteRejection::UnknownSegment(s(7)))
        );
        assert_eq!(
            validate(&[route(1, 2)], 1),
            Err(RouteRejection::NextHopIsLocal)
        );
        assert_eq!(
            validate(&[route(1, 3), route(1, 1)], 1),
            Err(RouteRejection::DuplicateDestination)
        );
        let mut internet = route(1, 3);
        internet.destination = RouteDestination::Internet;
        assert_eq!(
            validate(&[internet], 1),
            Err(RouteRejection::UnsupportedDestination)
        );
        let many = vec![route(1, 3); MAX_ASSIGNED_ROUTES + 1];
        assert_eq!(validate(&many, 1), Err(RouteRejection::TooManyRoutes));
        // One bad route rejects everything.
        assert!(validate(&[route(1, 3), route(2, 9)], 1).is_err());
    }

    #[test]
    fn failed_or_inactive_links_are_ignored_and_local_outside_the_graph_reaches_nothing() {
        let mut g = graph(
            &[(1, None), (2, Some(true)), (3, None)],
            &[(1, 1), (2, 2)],
            &[(1, 1, 2, 1), (2, 2, 3, 1)],
        );
        let unknown = n(42);
        let c = compute_routes(&g, unknown, network(), &[]);
        assert_eq!(c.unreachable.len(), 2);
        // Mark the S1 link failed by replacing it.
        g.add_link(PanLink {
            id: LinkId::from_bytes([1; 16]),
            segment_id: s(1),
            nap: n(1),
            panu: n(2),
            state: LinkState::Active,
            health: LinkHealth::Failed,
            cost: 1,
        })
        .unwrap();
        assert_eq!(
            compute_routes(&g, n(3), network(), &[]).unreachable,
            BTreeSet::from([s(1)])
        );
    }
}

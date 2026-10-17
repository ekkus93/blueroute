//! P8-008 software-only route recovery from an independently authenticated
//! and observed active topology. The caller must serialize topology changes
//! and route writes for the network, and the privileged backend must enforce
//! atomic ownership comparison at the actual kernel mutation.
use crate::{
    CoreError, NetworkId, NodeId, OwnedRouteOperator, RouteExecutionReport, TopologyGraph,
    desired_node_routes, execute_route_reconciliation,
};

/// Recompute next hops from *active observed* links and reconcile the result.
/// Unreachable destinations disappear rather than retaining a failed next hop.
/// An incomplete route mutation is not rolled back blindly: retry with fresh
/// topology and kernel observations to converge from the partial state.
pub fn reconcile_observed_topology_routes(
    graph: &TopologyGraph,
    source: NodeId,
    network: NetworkId,
    operator: &mut impl OwnedRouteOperator,
) -> Result<RouteExecutionReport, CoreError> {
    let desired = desired_node_routes(graph, source, network)?;
    execute_route_reconciliation(network, &desired, operator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CapabilitySource, ErrorKind, LinkHealth, LinkId, LinkState, NodeCapabilities, PanLink,
        PanSegment, Route, RouteDestination, RouteOwner, SegmentId, Sourced,
    };
    use std::collections::BTreeSet;

    fn id(n: u8) -> NodeId {
        NodeId::from_bytes([n; 16])
    }

    fn network(n: u8) -> NetworkId {
        NetworkId::from_bytes([n; 16])
    }

    fn add_link(graph: &mut TopologyGraph, id: u8, nap: u8, panu: u8, cost: u32) {
        graph
            .add_link(PanLink {
                id: LinkId::from_bytes([id; 16]),
                segment_id: SegmentId::from_bytes([nap; 16]),
                nap: self::id(nap),
                panu: self::id(panu),
                state: LinkState::Active,
                health: LinkHealth::Healthy,
                cost,
            })
            .unwrap();
    }

    fn topology() -> TopologyGraph {
        let mut graph = TopologyGraph::default();
        for n in 1..=4 {
            graph.add_node(
                id(n),
                NodeCapabilities {
                    routing: Some(Sourced::new(n == 2 || n == 3, CapabilitySource::Measured)),
                    ..NodeCapabilities::default()
                },
            );
        }
        for n in 1..=4 {
            graph
                .add_segment(PanSegment::new(SegmentId::from_bytes([n; 16]), id(n)))
                .unwrap();
        }
        // The alternate path is more expensive until the primary is lost.
        add_link(&mut graph, 1, 1, 2, 1);
        add_link(&mut graph, 2, 2, 4, 1);
        add_link(&mut graph, 3, 1, 3, 3);
        add_link(&mut graph, 4, 3, 4, 3);
        graph
    }

    struct ObservedRoutes {
        routes: Vec<Route>,
        operations: usize,
        fail_once_after_write: bool,
    }

    impl ObservedRoutes {
        fn new(routes: Vec<Route>) -> Self {
            Self {
                routes,
                operations: 0,
                fail_once_after_write: false,
            }
        }
    }

    impl OwnedRouteOperator for ObservedRoutes {
        fn observe_routes(&mut self) -> Result<Vec<Route>, CoreError> {
            Ok(self.routes.clone())
        }

        fn replace_owned_route(
            &mut self,
            previous: Option<&Route>,
            replacement: &Route,
        ) -> Result<(), CoreError> {
            if self
                .routes
                .iter()
                .find(|route| route.destination == replacement.destination)
                != previous
            {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "changed route owner",
                ));
            }
            self.routes
                .retain(|route| route.destination != replacement.destination);
            self.routes.push(replacement.clone());
            self.operations += 1;
            if self.fail_once_after_write {
                self.fail_once_after_write = false;
                return Err(CoreError::new(
                    ErrorKind::RouteFailure,
                    "injected partial write",
                ));
            }
            Ok(())
        }

        fn remove_owned_route(&mut self, previous: &Route) -> Result<(), CoreError> {
            if !self.routes.contains(previous) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "changed route owner",
                ));
            }
            self.routes.retain(|route| route != previous);
            self.operations += 1;
            Ok(())
        }
    }

    fn next_hop(routes: &[Route], node: u8) -> Option<NodeId> {
        routes.iter().find_map(|route| {
            if route.destination == RouteDestination::Node(id(node)) {
                Some(route.next_hop)
            } else {
                None
            }
        })
    }

    #[test]
    fn primary_loss_falls_back_then_partition_removes_and_restore_recovers() {
        let mut graph = topology();
        let mut backend = ObservedRoutes::new(Vec::new());
        let first =
            reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend).unwrap();
        assert_eq!(next_hop(&first.observed, 4), Some(id(2)));
        assert_eq!(first.observed.len(), 3);
        let initial_writes = backend.operations;
        assert!(
            reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend)
                .unwrap()
                .applied
                .is_empty()
        );
        assert_eq!(backend.operations, initial_writes);

        // Lose the primary second hop: the surviving higher-cost route wins.
        graph.remove_link(&LinkId::from_bytes([2; 16])).unwrap();
        let alternative =
            reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend).unwrap();
        assert_eq!(next_hop(&alternative.observed, 4), Some(id(3)));

        // The backup fails too: no invented reachability to node 4.
        graph.remove_link(&LinkId::from_bytes([4; 16])).unwrap();
        let partition =
            reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend).unwrap();
        assert_eq!(next_hop(&partition.observed, 4), None);
        assert_eq!(partition.observed.len(), 2);

        // The primary returns: reinstallation comes from the actual graph.
        add_link(&mut graph, 2, 2, 4, 1);
        let restored =
            reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend).unwrap();
        assert_eq!(next_hop(&restored.observed, 4), Some(id(2)));
        assert_eq!(restored.observed.len(), 3);
    }

    #[test]
    fn partial_failed_write_retries_from_observed_routes_and_keeps_foreign_network() {
        let graph = topology();
        let foreign = Route {
            destination: RouteDestination::Node(id(90)),
            next_hop: id(91),
            cost: 42,
            owner: RouteOwner::BlueRouteNetwork(network(8)),
        };
        let mut backend = ObservedRoutes::new(vec![foreign.clone()]);
        backend.fail_once_after_write = true;
        let failed = reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend);
        assert_eq!(failed.unwrap_err().kind(), ErrorKind::RouteFailure);
        assert!(backend.routes.contains(&foreign));
        assert_eq!(backend.routes.len(), 2);

        let recovered =
            reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend).unwrap();
        assert!(recovered.observed.contains(&foreign));
        assert_eq!(
            recovered
                .observed
                .into_iter()
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        assert_eq!(backend.operations, 3);
    }

    #[test]
    fn foreign_destination_collision_and_unknown_source_fail_closed() {
        let graph = topology();
        let foreign = Route {
            destination: RouteDestination::Node(id(4)),
            next_hop: id(90),
            cost: 42,
            owner: RouteOwner::BlueRouteNetwork(network(8)),
        };
        let mut backend = ObservedRoutes::new(vec![foreign.clone()]);
        assert_eq!(
            reconcile_observed_topology_routes(&graph, id(1), network(9), &mut backend)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
        assert!(
            reconcile_observed_topology_routes(&graph, id(0), network(9), &mut backend).is_err()
        );
        assert_eq!(backend.operations, 0);
        assert_eq!(backend.routes, vec![foreign]);
    }
}

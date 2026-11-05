//! Turn validated BlueRoute routes into Linux routes and reconcile them (P8-005).
//!
//! Only routes owned by the given `NetworkId` are ever considered installed. Routes owned by
//! anything else (the host, other networks, VPNs) are invisible to the planner and so can
//! never be removed. Stale BlueRoute routes (installed, owned, no longer desired) are removed.

use std::collections::BTreeMap;
use std::net::IpAddr;

use blueroute_core::{
    CoreError, ErrorKind, NetworkId, NodeId, Route, RouteDestination, SegmentSubnets,
};

use crate::{IpNetworkBackend, LinuxRoute, NetworkInterfaceHandle};

/// How to reach a next-hop node: its IP on a segment shared with the local node, and the local
/// interface attached to that segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextHopAddress {
    pub address: IpAddr,
    pub interface: NetworkInterfaceHandle,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct DesiredRoutes {
    pub routes: Vec<LinuxRoute>,
    /// Routes that could not be materialized yet (no subnet or next-hop address known).
    pub skipped: Vec<Route>,
}

/// Translate validated segment routes into Linux routes. Metric is the computed path cost.
pub fn to_linux_routes(
    routes: &[Route],
    network: NetworkId,
    subnets: &SegmentSubnets,
    next_hops: &BTreeMap<NodeId, NextHopAddress>,
) -> DesiredRoutes {
    let mut out = DesiredRoutes::default();
    for route in routes {
        let RouteDestination::Segment(segment) = &route.destination else {
            out.skipped.push(route.clone());
            continue;
        };
        match (subnets.get(segment), next_hops.get(&route.next_hop)) {
            (Some(destination), Some(hop)) => out.routes.push(LinuxRoute {
                destination,
                via: Some(hop.address),
                interface: hop.interface.clone(),
                metric: route.cost,
                owner: network,
            }),
            _ => out.skipped.push(route.clone()),
        }
    }
    out
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct RoutePlan {
    pub remove: Vec<LinuxRoute>,
    pub add: Vec<LinuxRoute>,
}

impl RoutePlan {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.add.is_empty()
    }
}

/// Compute the minimal change set. `installed` may contain foreign routes; they are ignored.
pub fn plan_route_changes(
    desired: &[LinuxRoute],
    installed: &[LinuxRoute],
    network: NetworkId,
) -> RoutePlan {
    let desired: Vec<&LinuxRoute> = desired.iter().filter(|r| r.owner == network).collect();
    let owned: Vec<&LinuxRoute> = installed.iter().filter(|r| r.owner == network).collect();
    let remove = owned
        .iter()
        .filter(|r| !desired.contains(r))
        .map(|r| (*r).clone())
        .collect();
    let mut add: Vec<LinuxRoute> = Vec::new();
    for route in desired {
        if !owned.contains(&route) && !add.contains(route) {
            add.push(route.clone());
        }
    }
    RoutePlan { remove, add }
}

/// Apply a plan: stale routes first (so a changed next hop for a destination never collides
/// with its old entry), then additions. Every step is attempted; failures are combined.
pub async fn apply_route_plan(
    backend: &dyn IpNetworkBackend,
    plan: RoutePlan,
) -> Result<(), CoreError> {
    let mut failures = Vec::new();
    for route in plan.remove {
        if let Err(e) = backend.remove_route(route.clone()).await {
            failures.push(format!("remove {}: {e}", route.destination.address));
        }
    }
    for route in plan.add {
        if let Err(e) = backend.ensure_route(route.clone()).await {
            failures.push(format!("add {}: {e}", route.destination.address));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CoreError::with_diagnostic(
            ErrorKind::RouteFailure,
            "failed to reconcile BlueRoute routes",
            failures.join("; "),
        ))
    }
}

/// Read installed routes, plan against `desired`, and apply. Idempotent.
pub async fn reconcile_routes(
    backend: &dyn IpNetworkBackend,
    desired: &[LinuxRoute],
    network: NetworkId,
) -> Result<RoutePlan, CoreError> {
    let installed = backend.routes().await?;
    let plan = plan_route_changes(desired, &installed, network);
    let summary = RoutePlan {
        remove: plan.remove.clone(),
        add: plan.add.clone(),
    };
    apply_route_plan(backend, plan).await?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::Mutex;

    use blueroute_core::{IpPrefix, Ipv4AddressPool, RouteOwner, SegmentId};
    use futures_lite::future::block_on;

    use super::*;
    use crate::{BackendFuture, InterfaceAddress};

    #[derive(Default)]
    struct FakeIp {
        routes: Mutex<Vec<LinuxRoute>>,
        fail_add: Mutex<bool>,
    }

    impl IpNetworkBackend for FakeIp {
        fn addresses(&self) -> BackendFuture<'_, Vec<InterfaceAddress>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn ensure_address(&self, _: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn remove_address(&self, _: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn routes(&self) -> BackendFuture<'_, Vec<LinuxRoute>> {
            Box::pin(async { Ok(self.routes.lock().unwrap().clone()) })
        }
        fn ensure_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                if *self.fail_add.lock().unwrap() {
                    return Err(CoreError::new(ErrorKind::RouteFailure, "boom"));
                }
                let mut routes = self.routes.lock().unwrap();
                if !routes.contains(&route) {
                    routes.push(route);
                }
                Ok(())
            })
        }
        fn remove_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                self.routes.lock().unwrap().retain(|r| r != &route);
                Ok(())
            })
        }
        fn set_ipv4_forwarding(&self, _: bool) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([5; 16])
    }

    fn iface() -> NetworkInterfaceHandle {
        NetworkInterfaceHandle::new("bnep0").unwrap()
    }

    fn prefix(third: u8) -> IpPrefix {
        IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 201, third, 0)), 24).unwrap()
    }

    fn linux(third: u8, hop: u8, owner: NetworkId) -> LinuxRoute {
        LinuxRoute {
            destination: prefix(third),
            via: Some(IpAddr::V4(Ipv4Addr::new(10, 201, 0, hop))),
            interface: iface(),
            metric: 2,
            owner,
        }
    }

    #[test]
    fn plan_adds_missing_removes_stale_and_never_touches_foreign_routes() {
        let foreign = linux(9, 9, NetworkId::from_bytes([0xEE; 16]));
        let stale = linux(1, 3, network());
        let keep = linux(2, 3, network());
        let new = linux(3, 3, network());
        let plan = plan_route_changes(
            &[keep.clone(), new.clone()],
            &[foreign.clone(), stale.clone(), keep],
            network(),
        );
        assert_eq!(plan.remove, vec![stale]);
        assert_eq!(plan.add, vec![new]);
        assert!(!plan.remove.contains(&foreign));
    }

    #[test]
    fn desired_routes_owned_by_another_network_are_never_planned() {
        let other = linux(1, 3, NetworkId::from_bytes([0xEE; 16]));
        assert!(plan_route_changes(&[other], &[], network()).is_empty());
    }

    #[test]
    fn changed_next_hop_replaces_the_old_route() {
        let old = linux(1, 3, network());
        let new = linux(1, 4, network());
        let plan = plan_route_changes(
            std::slice::from_ref(&new),
            std::slice::from_ref(&old),
            network(),
        );
        assert_eq!((plan.remove, plan.add), (vec![old], vec![new]));
    }

    #[test]
    fn route_table_matches_expected_topology_after_each_change() {
        block_on(async {
            let backend = FakeIp::default();
            let foreign = linux(9, 9, NetworkId::from_bytes([0xEE; 16]));
            backend.routes.lock().unwrap().push(foreign.clone());

            let first = [linux(1, 3, network()), linux(2, 3, network())];
            reconcile_routes(&backend, &first, network()).await.unwrap();
            let table = |b: &FakeIp| {
                let mut t = b.routes.lock().unwrap().clone();
                t.sort_by_key(|r| r.destination);
                t
            };
            assert_eq!(
                table(&backend),
                vec![first[0].clone(), first[1].clone(), foreign.clone()]
            );

            // Topology change: segment 1 now via another router, segment 2 gone.
            let second = [linux(1, 4, network())];
            reconcile_routes(&backend, &second, network())
                .await
                .unwrap();
            assert_eq!(table(&backend), vec![second[0].clone(), foreign.clone()]);

            // Idempotent: a second reconcile does nothing.
            assert!(
                reconcile_routes(&backend, &second, network())
                    .await
                    .unwrap()
                    .is_empty()
            );

            // Everything unreachable: all owned routes are removed, foreign remains.
            reconcile_routes(&backend, &[], network()).await.unwrap();
            assert_eq!(table(&backend), vec![foreign]);
        });
    }

    #[test]
    fn backend_failures_are_reported_after_attempting_every_step() {
        block_on(async {
            let backend = FakeIp::default();
            *backend.fail_add.lock().unwrap() = true;
            let error = reconcile_routes(&backend, &[linux(1, 3, network())], network())
                .await
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::RouteFailure);
            *backend.fail_add.lock().unwrap() = false;
            reconcile_routes(&backend, &[linux(1, 3, network())], network())
                .await
                .unwrap();
            assert_eq!(backend.routes.lock().unwrap().len(), 1);
        });
    }

    #[test]
    fn linux_routes_need_a_known_subnet_and_next_hop_address() {
        let mut subnets = SegmentSubnets::new(Ipv4AddressPool::default()).unwrap();
        let known = SegmentId::from_bytes([1; 16]);
        let prefix = subnets.allocate(known, &[]).unwrap();
        let hop = NodeId::from_bytes([3; 16]);
        let route = |seg, hop| Route {
            destination: RouteDestination::Segment(seg),
            next_hop: hop,
            cost: 4,
            owner: RouteOwner::BlueRouteNetwork(network()),
        };
        let addrs = BTreeMap::from([(
            hop,
            NextHopAddress {
                address: IpAddr::V4(Ipv4Addr::new(10, 201, 0, 7)),
                interface: iface(),
            },
        )]);
        let out = to_linux_routes(
            &[
                route(known, hop),
                route(SegmentId::from_bytes([2; 16]), hop),
                route(known, NodeId::from_bytes([8; 16])),
            ],
            network(),
            &subnets,
            &addrs,
        );
        assert_eq!(out.routes.len(), 1);
        assert_eq!(out.routes[0].destination, prefix);
        assert_eq!(out.routes[0].metric, 4);
        assert_eq!(out.skipped.len(), 2);
    }
}

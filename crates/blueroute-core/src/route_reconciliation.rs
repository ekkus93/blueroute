//! P8-005 backend-neutral desired/observed route reconciliation.
//!
//! This is a policy-only diff, not a kernel route writer or remote route
//! distribution protocol. Callers must independently authenticate topology,
//! map logical destinations to owned Linux prefixes/interfaces, and reobserve
//! the kernel before each privileged operation.
use std::collections::BTreeMap;

use crate::{
    CoreError, ErrorKind, NetworkId, Route, RouteDestination, RouteOwner,
};

/// A replacement of an existing destination is one upsert intent: the Linux
/// backend must use an ownership-checked atomic replace, not delete-then-add.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RouteReconciliationPlan {
    pub upsert: Vec<Route>,
    pub remove: Vec<Route>,
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidInput, message)
}

fn validate_desired(route: &Route, network: NetworkId) -> Result<(), CoreError> {
    if route.owner != RouteOwner::BlueRouteNetwork(network) {
        return Err(invalid("desired route belongs to another network"));
    }
    if route.next_hop.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(invalid("route next hop has zero identity"));
    }
    if route.cost == 0 {
        return Err(invalid("route cost must be positive"));
    }
    match &route.destination {
        RouteDestination::Internet => {
            Err(invalid("default/Internet route is outside local LAN policy"))
        }
        RouteDestination::Node(node) if node.as_bytes().iter().all(|byte| *byte == 0) => {
            Err(invalid("route destination has zero identity"))
        }
        RouteDestination::Segment(segment)
            if segment.as_bytes().iter().all(|byte| *byte == 0) =>
        {
            Err(invalid("route segment has zero identity"))
        }
        RouteDestination::Prefix(prefix) => {
            // IpPrefix has public fields; reject unchecked malformed values.
            crate::IpPrefix::new(prefix.address, prefix.prefix_len)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Produce deterministic route intents for one nonzero BlueRoute network.
///
/// Desired routes must all belong to this network and have unique destinations.
/// Observed routes for other networks are left untouched. Conflicting duplicate
/// owned observations fail closed instead of guessing which route to remove.
/// A changed next hop/cost is a single upsert (atomic backend replacement);
/// stale owned destinations are removed, including an unexpected owned default
/// route. No mutation occurs here.
pub fn plan_route_reconciliation(
    network: NetworkId,
    desired: &[Route],
    observed: &[Route],
) -> Result<RouteReconciliationPlan, CoreError> {
    if network.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(invalid("route reconciliation requires a nonzero network"));
    }
    let mut wanted = BTreeMap::new();
    for route in desired {
        validate_desired(route, network)?;
        if wanted.insert(route.destination.clone(), route.clone()).is_some() {
            return Err(invalid("duplicate desired route destination"));
        }
    }
    let mut existing = BTreeMap::new();
    for route in observed {
        if route.owner != RouteOwner::BlueRouteNetwork(network) {
            continue;
        }
        if existing.insert(route.destination.clone(), route.clone()).is_some() {
            return Err(invalid("duplicate observed owned route destination"));
        }
    }
    let mut plan = RouteReconciliationPlan::default();
    for (destination, route) in &wanted {
        if existing.get(destination) != Some(route) {
            plan.upsert.push(route.clone());
        }
    }
    for (destination, route) in existing {
        if !wanted.contains_key(&destination) {
            plan.remove.push(route);
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IpPrefix, NodeId};
    use std::net::{IpAddr, Ipv4Addr};

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn route(owner: u8, destination: u8, hop: u8, cost: u32) -> Route {
        Route {
            destination: RouteDestination::Node(NodeId::from_bytes([destination; 16])),
            next_hop: NodeId::from_bytes([hop; 16]),
            cost,
            owner: RouteOwner::BlueRouteNetwork(network(owner)),
        }
    }

    #[test]
    fn idempotent_diff_and_atomic_replacement_intent() {
        let old = route(1, 3, 2, 10);
        let new = route(1, 3, 4, 20);
        let stale = route(1, 5, 2, 10);
        let plan = plan_route_reconciliation(network(1), &[new.clone()], &[old, stale.clone()])
            .unwrap();
        assert_eq!(plan.upsert, vec![new.clone()]);
        assert_eq!(plan.remove, vec![stale]);
        assert_eq!(
            plan_route_reconciliation(network(1), &[new.clone()], &[new]).unwrap(),
            RouteReconciliationPlan::default()
        );
    }

    #[test]
    fn foreign_network_routes_are_preserved() {
        let foreign = route(2, 3, 7, 10);
        let local = route(1, 3, 2, 10);
        let plan = plan_route_reconciliation(network(1), &[local.clone()], &[foreign])
            .unwrap();
        assert_eq!(plan.upsert, vec![local]);
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn owned_unexpected_default_route_is_removed_not_reinstalled() {
        let mut unsafe_route = route(1, 3, 2, 10);
        unsafe_route.destination = RouteDestination::Internet;
        let plan = plan_route_reconciliation(network(1), &[], &[unsafe_route.clone()]).unwrap();
        assert_eq!(plan.remove, vec![unsafe_route.clone()]);
        assert!(plan.upsert.is_empty());
        assert!(plan_route_reconciliation(network(1), &[unsafe_route], &[]).is_err());
    }

    #[test]
    fn invalid_and_ambiguous_routes_fail_closed() {
        let valid = route(1, 3, 2, 10);
        assert!(plan_route_reconciliation(network(0), &[], &[]).is_err());
        assert!(plan_route_reconciliation(network(1), &[route(2, 3, 2, 10)], &[]).is_err());
        assert!(plan_route_reconciliation(network(1), &[valid.clone(), valid.clone()], &[]).is_err());
        assert!(plan_route_reconciliation(network(1), &[], &[valid.clone(), valid.clone()]).is_err());
        assert!(plan_route_reconciliation(network(1), &[route(1, 3, 0, 10)], &[]).is_err());
        assert!(plan_route_reconciliation(network(1), &[route(1, 3, 2, 0)], &[]).is_err());
        assert!(plan_route_reconciliation(network(1), &[route(1, 0, 2, 10)], &[]).is_err());
    }

    #[test]
    fn unchecked_invalid_prefix_is_rejected() {
        let mut bad = route(1, 3, 2, 10);
        bad.destination = RouteDestination::Prefix(IpPrefix {
            address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)),
            prefix_len: 33,
        });
        assert!(plan_route_reconciliation(network(1), &[bad], &[]).is_err());
    }
}

//! P8-005 backend-neutral desired/observed route reconciliation.
//!
//! This is a policy-only diff, not a kernel route writer or remote route
//! distribution protocol. Callers must independently authenticate topology,
//! map logical destinations to owned Linux prefixes/interfaces, and reobserve
//! the kernel before each privileged operation.
use std::collections::{BTreeMap, BTreeSet};

use crate::{CoreError, ErrorKind, NetworkId, Route, RouteDestination, RouteOwner};

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

/// Normalize the key that a Linux route backend would use when comparing
/// observed prefixes. This does not authorize a kernel mutation.
fn canonical_observed_destination(
    destination: &RouteDestination,
) -> Result<RouteDestination, CoreError> {
    let RouteDestination::Prefix(prefix) = destination else {
        return Ok(destination.clone());
    };
    crate::IpPrefix::new(prefix.address, prefix.prefix_len)?;
    let address = match prefix.address {
        std::net::IpAddr::V4(address) => {
            let mask = u32::MAX
                .checked_shl(u32::from(32 - prefix.prefix_len))
                .unwrap_or(0);
            std::net::IpAddr::V4(std::net::Ipv4Addr::from(u32::from(address) & mask))
        }
        std::net::IpAddr::V6(address) => {
            let mask = u128::MAX
                .checked_shl(u32::from(128 - prefix.prefix_len))
                .unwrap_or(0);
            std::net::IpAddr::V6(std::net::Ipv6Addr::from(u128::from(address) & mask))
        }
    };
    Ok(RouteDestination::Prefix(crate::IpPrefix::new(
        address,
        prefix.prefix_len,
    )?))
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
        RouteDestination::Internet => Err(invalid(
            "default/Internet route is outside local LAN policy",
        )),
        RouteDestination::Node(node) if node.as_bytes().iter().all(|byte| *byte == 0) => {
            Err(invalid("route destination has zero identity"))
        }
        RouteDestination::Segment(segment) if segment.as_bytes().iter().all(|byte| *byte == 0) => {
            Err(invalid("route segment has zero identity"))
        }
        RouteDestination::Prefix(prefix) => {
            // IpPrefix has public fields; reject unchecked malformed values.
            crate::IpPrefix::new(prefix.address, prefix.prefix_len)?;
            if prefix.prefix_len == 0 {
                return Err(invalid("zero-length prefix is not a local route"));
            }
            // The Linux kernel canonicalizes destination prefixes. Reject
            // host bits instead of treating different encodings of the same
            // destination as independently owned routes.
            let canonical = match prefix.address {
                std::net::IpAddr::V4(address) => {
                    let host_bits = 32 - u32::from(prefix.prefix_len);
                    let bits = u32::from(address);
                    bits & (u32::MAX << host_bits) == bits
                }
                std::net::IpAddr::V6(address) => {
                    let host_bits = 128 - u32::from(prefix.prefix_len);
                    let bits = u128::from(address);
                    bits & (u128::MAX << host_bits) == bits
                }
            };
            if !canonical {
                return Err(invalid("route prefix contains noncanonical host bits"));
            }
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
        if wanted
            .insert(route.destination.clone(), route.clone())
            .is_some()
        {
            return Err(invalid("duplicate desired route destination"));
        }
    }
    let mut existing = BTreeMap::new();
    let mut foreign_destinations = BTreeSet::new();
    for route in observed {
        // The kernel may report a route with its network address normalized,
        // while an observation adapter could preserve host bits. Compare the
        // effective destination rather than the unchecked text representation.
        let destination = canonical_observed_destination(&route.destination)?;
        if route.owner != RouteOwner::BlueRouteNetwork(network) {
            // A backend must never replace a route owned by another network,
            // including a noncanonical alias for the same kernel prefix.
            if wanted.contains_key(&destination) {
                return Err(invalid("desired destination conflicts with foreign route"));
            }
            foreign_destinations.insert(destination);
            continue;
        }
        // Never infer ownership of a different canonical route from a
        // malformed owned observation.
        if destination != route.destination {
            return Err(invalid("observed owned route has noncanonical prefix"));
        }
        if existing.insert(destination, route.clone()).is_some() {
            return Err(invalid("duplicate observed owned route destination"));
        }
    }
    // A stale owned route is not safe to delete when a foreign observation
    // aliases the same effective kernel destination. Validate independently
    // of observation order, including when no desired route remains.
    if existing.keys().any(|key| foreign_destinations.contains(key)) {
        return Err(invalid("owned destination conflicts with foreign route"));
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
        let desired = [new.clone()];
        let plan = plan_route_reconciliation(network(1), &desired, &[old, stale.clone()]).unwrap();
        assert_eq!(plan.upsert, vec![new.clone()]);
        assert_eq!(plan.remove, vec![stale]);
        assert_eq!(
            plan_route_reconciliation(network(1), &desired, &[new]).unwrap(),
            RouteReconciliationPlan::default()
        );
    }

    #[test]
    fn foreign_network_routes_are_preserved() {
        let foreign = route(2, 4, 7, 10);
        let local = route(1, 3, 2, 10);
        let desired = [local.clone()];
        let plan = plan_route_reconciliation(network(1), &desired, &[foreign]).unwrap();
        assert_eq!(plan.upsert, vec![local]);
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn foreign_route_collision_fails_closed_without_planning_replacement() {
        let foreign = route(2, 3, 7, 10);
        let desired = route(1, 3, 2, 10);
        assert!(plan_route_reconciliation(network(1), &[desired], &[foreign]).is_err());
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
        assert!(
            plan_route_reconciliation(network(1), &[valid.clone(), valid.clone()], &[]).is_err()
        );
        assert!(
            plan_route_reconciliation(network(1), &[], &[valid.clone(), valid.clone()]).is_err()
        );
        assert!(plan_route_reconciliation(network(1), &[route(1, 3, 0, 10)], &[]).is_err());
        assert!(plan_route_reconciliation(network(1), &[route(1, 3, 2, 0)], &[]).is_err());
        assert!(plan_route_reconciliation(network(1), &[route(1, 0, 2, 10)], &[]).is_err());
    }

    #[test]
    fn noncanonical_prefixes_cannot_alias_kernel_destinations() {
        use std::net::Ipv6Addr;

        let mut desired = route(1, 3, 2, 10);
        desired.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 1)), 24).unwrap(),
        );
        assert!(
            plan_route_reconciliation(network(1), std::slice::from_ref(&desired), &[]).is_err()
        );

        desired.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 0)), 24).unwrap(),
        );
        assert_eq!(
            plan_route_reconciliation(network(1), std::slice::from_ref(&desired), &[])
                .unwrap()
                .upsert,
            vec![desired.clone()]
        );

        desired.destination = RouteDestination::Prefix(
            IpPrefix::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 1, 0, 0, 0, 1)),
                64,
            )
            .unwrap(),
        );
        assert!(
            plan_route_reconciliation(network(1), std::slice::from_ref(&desired), &[]).is_err()
        );

        desired.destination = RouteDestination::Prefix(
            IpPrefix::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 1, 0, 0, 0, 0)),
                64,
            )
            .unwrap(),
        );
        assert_eq!(
            plan_route_reconciliation(network(1), std::slice::from_ref(&desired), &[])
                .unwrap()
                .upsert,
            vec![desired]
        );
    }

    #[test]
    fn foreign_noncanonical_ipv4_prefix_cannot_hide_kernel_collision() {
        let mut desired = route(1, 3, 2, 10);
        desired.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 0)), 24).unwrap(),
        );
        let mut foreign = route(2, 4, 7, 10);
        foreign.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 77)), 24).unwrap(),
        );
        assert!(

            plan_route_reconciliation(network(1), &[desired.clone()], &[foreign.clone()]).is_err()
        );

        foreign.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 21, 0, 77)), 24).unwrap(),
        );
        assert_eq!(
            plan_route_reconciliation(network(1), &[desired.clone()], &[foreign])
                .unwrap()
                .upsert,
            vec![desired]
        );
    }

    #[test]
    fn foreign_noncanonical_ipv6_prefix_cannot_hide_kernel_collision() {
        use std::net::Ipv6Addr;

        let mut desired = route(1, 3, 2, 10);
        desired.destination = RouteDestination::Prefix(
            IpPrefix::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, 0)),
                64,
            )
            .unwrap(),
        );
        let mut foreign = route(2, 4, 7, 10);
        foreign.destination = RouteDestination::Prefix(
            IpPrefix::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, 9)),
                64,
            )
            .unwrap(),
        );
        assert!(plan_route_reconciliation(network(1), &[desired], &[foreign]).is_err());
    }

    #[test]
    fn stale_owned_prefix_cannot_be_removed_under_foreign_alias() {
        let mut owned = route(1, 3, 2, 10);
        owned.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 0)), 24).unwrap(),
        );
        let mut foreign = route(2, 4, 7, 10);
        foreign.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 77)), 24).unwrap(),
        );
        // Neither observation order may authorize removal of an ambiguous
        // kernel prefix, even though the desired route set is empty.
        for observations in [
            [owned.clone(), foreign.clone()],
            [foreign.clone(), owned.clone()],
        ] {
            assert!(plan_route_reconciliation(network(1), &[], &observations).is_err());
        }
        foreign.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 21, 0, 77)), 24).unwrap(),
        );
        assert_eq!(
            plan_route_reconciliation(network(1), &[], &[owned.clone(), foreign])
                .unwrap()
                .remove,
            vec![owned]
        );
    }

    #[test]
    fn malformed_owned_observation_cannot_authorize_mutation() {
        let mut owned = route(1, 3, 2, 10);
        owned.destination = RouteDestination::Prefix(
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 77)), 24).unwrap(),
        );
        assert!(plan_route_reconciliation(network(1), &[], &[owned.clone()]).is_err());
        if let RouteDestination::Prefix(prefix) = &mut owned.destination {
            prefix.prefix_len = 33;
        }
        assert!(plan_route_reconciliation(network(1), &[], &[owned]).is_err());
    }

    #[test]
    fn unchecked_invalid_prefix_is_rejected() {
        let mut bad = route(1, 3, 2, 10);
        bad.destination = RouteDestination::Prefix(IpPrefix {
            address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)),
            prefix_len: 33,
        });
        assert!(plan_route_reconciliation(network(1), &[bad.clone()], &[]).is_err());
        if let RouteDestination::Prefix(prefix) = &mut bad.destination {
            prefix.prefix_len = 0;
        }
        assert!(plan_route_reconciliation(network(1), &[bad], &[]).is_err());
    }
}

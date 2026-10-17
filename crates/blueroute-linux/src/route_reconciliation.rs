//! Software-only P8-005 desired/observed Linux route planning.
//!
//! No privileged kernel mutations occur here. The privileged backend must
//! independently prove table, prefix and interface ownership at the instant
//! of each atomic replace/remove. Its observation must include foreign routes,
//! not only the owned-route projection from `IpNetworkBackend::routes`.
use std::collections::BTreeMap;
use std::net::IpAddr;

use blueroute_core::{CoreError, ErrorKind, IpPrefix, NetworkId};

use crate::LinuxRoute;
use crate::route_projection::private_prefix;

/// Atomic replacement intents and stale owner-scoped removal intents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinuxRouteReconciliationPlan {
    pub upsert: Vec<LinuxRoute>,
    pub remove: Vec<LinuxRoute>,
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidInput, message)
}

/// Normalize an observed prefix to the destination that Linux would use.
/// A foreign observation is not trusted merely because it has an owner tag.
fn canonical(prefix: IpPrefix) -> Result<IpPrefix, CoreError> {
    IpPrefix::new(prefix.address, prefix.prefix_len)?;
    let address = match prefix.address {
        IpAddr::V4(address) => {
            let mask = u32::MAX
                .checked_shl(u32::from(32 - prefix.prefix_len))
                .unwrap_or(0);
            IpAddr::V4(std::net::Ipv4Addr::from(u32::from(address) & mask))
        }
        IpAddr::V6(address) => {
            let mask = u128::MAX
                .checked_shl(u32::from(128 - prefix.prefix_len))
                .unwrap_or(0);
            IpAddr::V6(std::net::Ipv6Addr::from(u128::from(address) & mask))
        }
    };
    IpPrefix::new(address, prefix.prefix_len)
}

fn validate_owned(route: &LinuxRoute, network: NetworkId) -> Result<(), CoreError> {
    if route.owner != network {
        return Err(invalid("Linux route is not owned by the requested network"));
    }
    private_prefix(route.destination)?;
    if route.metric == 0 {
        return Err(invalid("Linux route metric must be positive"));
    }
    if let Some(via) = route.via {
        private_prefix(IpPrefix::new(via, 32)?)?;
        if route.destination.prefix_len == 32 && route.destination.address == via {
            return Err(invalid("Linux host route cannot forward via itself"));
        }
    }
    Ok(())
}

/// Plan changes for exactly one trusted NetworkId. Foreign routes are never
/// removed or replaced. Invalid owned state, ambiguous destinations and
/// noncanonical aliases fail closed rather than guessing kernel ownership.
pub fn plan_linux_route_reconciliation(
    network: NetworkId,
    desired: &[LinuxRoute],
    observed: &[LinuxRoute],
) -> Result<LinuxRouteReconciliationPlan, CoreError> {
    if network.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(invalid(
            "Linux route reconciliation requires a network identity",
        ));
    }
    let mut wanted = BTreeMap::new();
    for route in desired {
        validate_owned(route, network)?;
        if wanted.insert(route.destination, route.clone()).is_some() {
            return Err(invalid("duplicate desired Linux route destination"));
        }
    }

    let mut existing = BTreeMap::new();
    for route in observed {
        let key = canonical(route.destination)?;
        if route.owner != network {
            if wanted.contains_key(&key) {
                return Err(invalid("desired Linux route collides with foreign route"));
            }
            continue;
        }
        validate_owned(route, network)?;
        if route.destination != key {
            return Err(invalid("owned Linux route has noncanonical destination"));
        }
        if existing.insert(key, route.clone()).is_some() {
            return Err(invalid("duplicate observed owned Linux route destination"));
        }
    }

    let mut plan = LinuxRouteReconciliationPlan::default();
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
    use crate::NetworkInterfaceHandle;
    use std::net::Ipv4Addr;

    fn network(n: u8) -> NetworkId {
        NetworkId::from_bytes([n; 16])
    }

    fn route(owner: u8, address: [u8; 4], hop: Option<[u8; 4]>) -> LinuxRoute {
        LinuxRoute {
            destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::from(address)), 32).unwrap(),
            via: hop.map(|value| IpAddr::V4(Ipv4Addr::from(value))),
            interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
            metric: 10,
            owner: network(owner),
        }
    }

    #[test]
    fn deterministically_replaces_and_removes_owned_routes() {
        let old = route(1, [10, 20, 0, 3], Some([10, 20, 0, 2]));
        let mut desired = old.clone();
        desired.metric = 20;
        let stale = route(1, [10, 20, 0, 9], None);
        let foreign = route(2, [10, 30, 0, 3], None);
        let plan = plan_linux_route_reconciliation(
            network(1),
            &[desired.clone()],
            &[old, stale.clone(), foreign],
        )
        .unwrap();
        assert_eq!(plan.upsert, vec![desired.clone()]);
        assert_eq!(plan.remove, vec![stale]);
        assert_eq!(
            plan_linux_route_reconciliation(network(1), &[desired.clone()], &[desired]).unwrap(),
            LinuxRouteReconciliationPlan::default()
        );
    }

    #[test]
    fn foreign_observed_kernel_alias_blocks_replacement() {
        let desired = route(1, [10, 20, 0, 0], Some([10, 30, 0, 2]));
        let mut desired = desired;
        desired.destination = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 0)), 24).unwrap();
        let mut foreign = route(2, [10, 20, 0, 77], None);
        foreign.destination = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 77)), 24).unwrap();
        assert!(plan_linux_route_reconciliation(network(1), &[desired], &[foreign]).is_err());
    }

    #[test]
    fn foreign_unrelated_routes_are_unchanged_and_not_removed() {
        let foreign = route(2, [10, 30, 0, 3], None);
        let desired = route(1, [10, 20, 0, 3], Some([10, 20, 0, 2]));
        let plan =
            plan_linux_route_reconciliation(network(1), &[desired.clone()], &[foreign]).unwrap();
        assert_eq!(plan.upsert, vec![desired]);
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn invalid_ownership_and_ambiguous_observations_fail_closed() {
        let desired = route(1, [10, 20, 0, 3], Some([10, 20, 0, 2]));
        let foreign = route(2, [10, 20, 0, 3], None);
        assert!(
            plan_linux_route_reconciliation(network(1), &[desired.clone()], &[foreign]).is_err()
        );
        assert!(
            plan_linux_route_reconciliation(network(1), &[desired.clone(), desired.clone()], &[])
                .is_err()
        );
        assert!(
            plan_linux_route_reconciliation(network(1), &[], &[desired.clone(), desired.clone()])
                .is_err()
        );
        assert!(plan_linux_route_reconciliation(network(0), &[], &[]).is_err());
        let mut bad = desired;
        bad.destination = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 7)), 24).unwrap();
        assert!(plan_linux_route_reconciliation(network(1), &[], &[bad]).is_err());
    }

    #[test]
    fn invalid_gateway_default_and_self_route_never_plan() {
        let mut desired = route(1, [10, 20, 0, 3], Some([10, 20, 0, 2]));
        desired.via = Some(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
        assert!(plan_linux_route_reconciliation(network(1), &[desired.clone()], &[]).is_err());
        desired.via = Some(IpAddr::V4(Ipv4Addr::new(10, 20, 0, 3)));
        assert!(plan_linux_route_reconciliation(network(1), &[desired.clone()], &[]).is_err());
        desired.via = None;
        desired.destination = IpPrefix::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).unwrap();
        assert!(plan_linux_route_reconciliation(network(1), &[desired], &[]).is_err());
    }
}

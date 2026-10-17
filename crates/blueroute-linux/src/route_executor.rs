//! P8-005: observation-gated, backend-neutral Linux route execution.
//!
//! No implementation in this module writes the kernel route table. The
//! privileged operator must observe the complete relevant routing table
//! (including foreign/unowned routes) and atomically validate ownership,
//! destination, interface and previous state at the mutation site. An unknown
//! or unverifiable ownership classification must fail closed.

use blueroute_core::{CoreError, ErrorKind, NetworkId};

use crate::{LinuxRoute, plan_linux_route_reconciliation};

/// Privileged backend contract. These methods are not satisfied merely by
/// the existing IpNetworkBackend owned-only route enumeration.
/// A real implementation must serialize operations by network and enforce
/// compare-and-swap under its own kernel mutation/ownership boundary.
pub trait LinuxOwnedRouteOperator {
    /// Complete, fresh, trustworthy observation, never a requested-state cache.
    fn observe_all_routes(&mut self) -> Result<Vec<LinuxRoute>, CoreError>;

    /// Replace only when the exact previous owned route still exists, or
    /// create only when the destination is still unoccupied.
    fn replace_if_current(
        &mut self,
        previous: Option<&LinuxRoute>,
        replacement: &LinuxRoute,
    ) -> Result<(), CoreError>;

    /// Remove only the exact current owned route, never a foreign replacement.
    fn remove_if_current(&mut self, previous: &LinuxRoute) -> Result<(), CoreError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinuxRouteChange {
    Upsert(LinuxRoute),
    Remove(LinuxRoute),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxRouteExecutionReport {
    pub applied: Vec<LinuxRouteChange>,
    pub observed: Vec<LinuxRoute>,
}

fn stale() -> CoreError {
    CoreError::new(
        ErrorKind::InvalidState,
        "Linux routing state changed during reconciliation; replan",
    )
}

fn snapshot_sort(routes: &mut [LinuxRoute]) {
    routes.sort_by(|left, right| {
        (
            left.destination,
            &left.interface,
            left.via,
            left.metric,
            left.owner.as_bytes(),
        )
            .cmp(&(
                right.destination,
                &right.interface,
                right.via,
                right.metric,
                right.owner.as_bytes(),
            ))
    });
}

/// Execute a single plan using fresh observations and exact-state checks.
/// Do not roll back blindly on a partial failure: the next attempt must
/// reobserve the true routing table and compute a new plan.
///
/// An observation check alone cannot prevent races. Only a backend that
/// atomically validates the expected previous state and ownership at the
/// privileged write boundary is permitted to implement this trait.
pub fn execute_linux_route_reconciliation(
    network: NetworkId,
    desired: &[LinuxRoute],
    backend: &mut impl LinuxOwnedRouteOperator,
) -> Result<LinuxRouteExecutionReport, CoreError> {
    let mut observed = backend.observe_all_routes()?;
    let plan = plan_linux_route_reconciliation(network, desired, &observed)?;
    snapshot_sort(&mut observed);
    let mut applied = Vec::new();

    for next in plan.upsert {
        let mut fresh = backend.observe_all_routes()?;
        snapshot_sort(&mut fresh);
        if fresh != observed {
            return Err(stale());
        }
        let previous = observed
            .iter()
            .find(|route| route.owner == network && route.destination == next.destination);
        backend.replace_if_current(previous, &next)?;

        let mut expected = observed.clone();
        expected.retain(|route| !(route.owner == network && route.destination == next.destination));
        expected.push(next.clone());
        snapshot_sort(&mut expected);
        let mut post = backend.observe_all_routes()?;
        snapshot_sort(&mut post);
        if post != expected {
            return Err(CoreError::new(
                ErrorKind::RouteFailure,
                "Linux route replacement did not produce the observed expected state",
            ));
        }
        observed = post;
        applied.push(LinuxRouteChange::Upsert(next));
    }

    for old in plan.remove {
        let mut fresh = backend.observe_all_routes()?;
        snapshot_sort(&mut fresh);
        if fresh != observed {
            return Err(stale());
        }
        backend.remove_if_current(&old)?;
        let mut expected = observed.clone();
        expected.retain(|route| route != &old);
        let mut post = backend.observe_all_routes()?;
        snapshot_sort(&mut post);
        if post != expected {
            return Err(CoreError::new(
                ErrorKind::RouteFailure,
                "Linux route removal did not produce the observed expected state",
            ));
        }
        observed = post;
        applied.push(LinuxRouteChange::Remove(old));
    }

    Ok(LinuxRouteExecutionReport { applied, observed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    use blueroute_core::IpPrefix;

    use crate::NetworkInterfaceHandle;

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn route(owner: u8, destination: u8, next_hop: u8) -> LinuxRoute {
        LinuxRoute {
            destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 20, 0, destination)), 32)
                .unwrap(),
            via: Some(IpAddr::V4(Ipv4Addr::new(10, 20, 0, next_hop))),
            interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
            metric: 10,
            owner: network(owner),
        }
    }

    struct FakeKernel {
        routes: Vec<LinuxRoute>,
        writes: Vec<LinuxRouteChange>,
        reads: usize,
        inject_on_read: Option<(usize, LinuxRoute)>,
        fail_after_write: bool,
        ignore_write: bool,
        race_before_write: bool,
    }

    impl FakeKernel {
        fn new(routes: Vec<LinuxRoute>) -> Self {
            Self {
                routes,
                writes: vec![],
                reads: 0,
                inject_on_read: None,
                fail_after_write: false,
                ignore_write: false,
                race_before_write: false,
            }
        }
    }

    impl LinuxOwnedRouteOperator for FakeKernel {
        fn observe_all_routes(&mut self) -> Result<Vec<LinuxRoute>, CoreError> {
            self.reads += 1;
            if self
                .inject_on_read
                .as_ref()
                .is_some_and(|(at, _)| *at == self.reads)
            {
                self.routes.push(self.inject_on_read.take().unwrap().1);
            }
            Ok(self.routes.clone())
        }

        fn replace_if_current(
            &mut self,
            previous: Option<&LinuxRoute>,
            replacement: &LinuxRoute,
        ) -> Result<(), CoreError> {
            if self.race_before_write {
                return Err(stale());
            }
            let actual = self
                .routes
                .iter()
                .find(|route| route.destination == replacement.destination);
            if actual != previous || previous.is_some_and(|route| route.owner != replacement.owner)
            {
                return Err(stale());
            }
            if !self.ignore_write {
                self.routes
                    .retain(|route| route.destination != replacement.destination);
                self.routes.push(replacement.clone());
            }
            self.writes
                .push(LinuxRouteChange::Upsert(replacement.clone()));
            if self.fail_after_write {
                return Err(CoreError::new(
                    ErrorKind::RouteFailure,
                    "injected failure after write",
                ));
            }
            Ok(())
        }

        fn remove_if_current(&mut self, previous: &LinuxRoute) -> Result<(), CoreError> {
            if self.race_before_write
                || !self.routes.contains(previous)
                || self
                    .routes
                    .iter()
                    .any(|route| route.destination == previous.destination && route != previous)
            {
                return Err(stale());
            }
            if !self.ignore_write {
                self.routes.retain(|route| route != previous);
            }
            self.writes.push(LinuxRouteChange::Remove(previous.clone()));
            if self.fail_after_write {
                return Err(CoreError::new(
                    ErrorKind::RouteFailure,
                    "injected failure after write",
                ));
            }
            Ok(())
        }
    }

    #[test]
    fn applies_replacement_then_removal_and_preserves_foreign_routes() {
        let foreign = route(2, 90, 91);
        let stale_route = route(1, 15, 2);
        let old = route(1, 3, 2);
        let mut updated = old.clone();
        updated.metric = 20;
        let mut backend = FakeKernel::new(vec![foreign.clone(), stale_route.clone(), old]);
        let report = execute_linux_route_reconciliation(
            network(1),
            std::slice::from_ref(&updated),
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            report.applied,
            vec![
                LinuxRouteChange::Upsert(updated.clone()),
                LinuxRouteChange::Remove(stale_route),
            ]
        );
        assert!(report.observed.contains(&foreign));
        assert!(report.observed.contains(&updated));
        assert!(
            execute_linux_route_reconciliation(network(1), &[updated], &mut backend)
                .unwrap()
                .applied
                .is_empty()
        );
        assert_eq!(backend.writes.len(), 2);
    }

    #[test]
    fn foreign_destination_collision_rejects_before_writes() {
        let mut backend = FakeKernel::new(vec![route(2, 3, 9)]);
        assert_eq!(
            execute_linux_route_reconciliation(network(1), &[route(1, 3, 2)], &mut backend)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
        assert!(backend.writes.is_empty());
    }

    #[test]
    fn external_observation_change_stops_before_first_write() {
        let unexpected = route(2, 90, 91);
        let mut backend = FakeKernel::new(vec![]);
        backend.inject_on_read = Some((2, unexpected.clone()));
        assert_eq!(
            execute_linux_route_reconciliation(network(1), &[route(1, 3, 2)], &mut backend)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(backend.routes, vec![unexpected]);
        assert!(backend.writes.is_empty());
    }

    #[test]
    fn privileged_compare_and_swap_failure_stops_without_writes() {
        let mut backend = FakeKernel::new(vec![]);
        backend.race_before_write = true;
        assert_eq!(
            execute_linux_route_reconciliation(network(1), &[route(1, 3, 2)], &mut backend)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        assert!(backend.writes.is_empty());
    }

    #[test]
    fn partial_write_failure_replans_from_actual_state() {
        let first = route(1, 3, 2);
        let second = route(1, 4, 2);
        let mut backend = FakeKernel::new(vec![]);
        backend.fail_after_write = true;
        assert!(
            execute_linux_route_reconciliation(
                network(1),
                &[first.clone(), second.clone()],
                &mut backend,
            )
            .is_err()
        );
        assert_eq!(backend.routes, vec![first.clone()]);
        backend.fail_after_write = false;
        let report =
            execute_linux_route_reconciliation(network(1), &[first, second.clone()], &mut backend)
                .unwrap();
        assert_eq!(report.applied, vec![LinuxRouteChange::Upsert(second)]);
    }

    #[test]
    fn reported_success_without_effect_is_detected() {
        let mut backend = FakeKernel::new(vec![]);
        backend.ignore_write = true;
        assert_eq!(
            execute_linux_route_reconciliation(network(1), &[route(1, 3, 2)], &mut backend)
                .unwrap_err()
                .kind(),
            ErrorKind::RouteFailure
        );
        assert_eq!(backend.writes.len(), 1);
    }

    #[test]
    fn change_before_second_operation_preserves_partial_progress() {
        let unexpected = route(2, 90, 91);
        let first = route(1, 3, 2);
        let second = route(1, 4, 2);
        let mut backend = FakeKernel::new(vec![]);
        backend.inject_on_read = Some((4, unexpected.clone()));
        assert_eq!(
            execute_linux_route_reconciliation(network(1), &[first.clone(), second], &mut backend,)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(
            backend.writes,
            vec![LinuxRouteChange::Upsert(first.clone())]
        );
        assert!(backend.routes.contains(&unexpected));
        assert!(backend.routes.contains(&first));
    }
}

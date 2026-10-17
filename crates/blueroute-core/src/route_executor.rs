//! P8-005 observation-gated application of route reconciliation intents.
//!
//! This software-only contract does not mutate Linux kernel routes. The
//! privileged backend MUST atomically compare route ownership/destination
//! against the supplied previous value before replacing or deleting anything.
//! An authenticated per-network owner must serialize callers, and must not
//! pass unauthenticated topology claims to this executor.
use crate::{CoreError, ErrorKind, NetworkId, Route, RouteOwner, plan_route_reconciliation};

/// Observations must reflect kernel state, not cached successful requests.
/// The backend guarantees atomic ownership checks *at the mutation itself*;
/// the pre-operation observation cannot eliminate a concurrent kernel race.
pub trait OwnedRouteOperator {
    fn observe_routes(&mut self) -> Result<Vec<Route>, CoreError>;
    fn replace_owned_route(
        &mut self,
        previous: Option<&Route>,
        replacement: &Route,
    ) -> Result<(), CoreError>;
    fn remove_owned_route(&mut self, previous: &Route) -> Result<(), CoreError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteChange {
    Upsert(Route),
    Remove(Route),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteExecutionReport {
    pub applied: Vec<RouteChange>,
    pub observed: Vec<Route>,
}

fn stale() -> CoreError {
    CoreError::new(
        ErrorKind::InvalidState,
        "observed routes changed during reconciliation; replan",
    )
}

/// Execute an ownership-checked deterministic route plan.
///
/// Upserts are applied before removals, so viable existing destinations are
/// not deleted before an alternative is installed. Each operation is preceded
/// by a fresh observation and followed by an exact observation verification
/// (including foreign routes). The backend must independently compare-and-swap
/// route ownership under its privileged mutation lock. On any failure, stop:
/// the next caller must reobserve and plan from the actual resulting routes.
pub fn execute_route_reconciliation(
    network: NetworkId,
    desired: &[Route],
    operator: &mut impl OwnedRouteOperator,
) -> Result<RouteExecutionReport, CoreError> {
    let mut observed = operator.observe_routes()?;
    let plan = plan_route_reconciliation(network, desired, &observed)?;
    observed.sort();
    let mut applied = Vec::new();
    let owner = RouteOwner::BlueRouteNetwork(network);

    for new in plan.upsert {
        let mut actual = operator.observe_routes()?;
        actual.sort();
        if actual != observed {
            return Err(stale());
        }
        let previous = observed
            .iter()
            .find(|route| route.owner == owner && route.destination == new.destination);
        operator.replace_owned_route(previous, &new)?;
        let mut expected = observed.clone();
        expected.retain(|route| !(route.owner == owner && route.destination == new.destination));
        expected.push(new.clone());
        expected.sort();
        let mut post = operator.observe_routes()?;
        post.sort();
        if post != expected {
            return Err(CoreError::new(
                ErrorKind::RouteFailure,
                "route upsert did not produce the exact observed state",
            ));
        }
        observed = post;
        applied.push(RouteChange::Upsert(new));
    }

    for old in plan.remove {
        let mut actual = operator.observe_routes()?;
        actual.sort();
        if actual != observed {
            return Err(stale());
        }
        operator.remove_owned_route(&old)?;
        let mut expected = observed.clone();
        expected.retain(|route| route != &old);
        let mut post = operator.observe_routes()?;
        post.sort();
        if post != expected {
            return Err(CoreError::new(
                ErrorKind::RouteFailure,
                "route removal did not produce the exact observed state",
            ));
        }
        observed = post;
        applied.push(RouteChange::Remove(old));
    }
    Ok(RouteExecutionReport { applied, observed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeId, RouteDestination};
    use std::collections::BTreeSet;

    fn network(n: u8) -> NetworkId {
        NetworkId::from_bytes([n; 16])
    }

    fn route(net: u8, destination: u8, hop: u8) -> Route {
        Route {
            destination: RouteDestination::Node(NodeId::from_bytes([destination; 16])),
            next_hop: NodeId::from_bytes([hop; 16]),
            cost: 10,
            owner: RouteOwner::BlueRouteNetwork(network(net)),
        }
    }

    struct FakeRoutes {
        routes: Vec<Route>,
        writes: Vec<RouteChange>,
        observations: usize,
        mutate_on_observe: Option<(usize, Route)>,
        fail_after_write: bool,
        ignore_write: bool,
    }

    impl FakeRoutes {
        fn new(routes: Vec<Route>) -> Self {
            Self {
                routes,
                writes: Vec::new(),
                observations: 0,
                mutate_on_observe: None,
                fail_after_write: false,
                ignore_write: false,
            }
        }
    }

    impl OwnedRouteOperator for FakeRoutes {
        fn observe_routes(&mut self) -> Result<Vec<Route>, CoreError> {
            self.observations += 1;
            if self
                .mutate_on_observe
                .as_ref()
                .is_some_and(|(at, _)| self.observations == *at)
            {
                self.routes.push(self.mutate_on_observe.take().unwrap().1);
            }
            Ok(self.routes.clone())
        }

        fn replace_owned_route(
            &mut self,
            previous: Option<&Route>,
            replacement: &Route,
        ) -> Result<(), CoreError> {
            let prior = self
                .routes
                .iter()
                .find(|route| route.destination == replacement.destination);
            if prior != previous || replacement.owner == RouteOwner::BlueRouteNetwork(network(0)) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "kernel compare-and-swap route ownership mismatch",
                ));
            }
            if !self.ignore_write {
                self.routes
                    .retain(|route| route.destination != replacement.destination);
                self.routes.push(replacement.clone());
            }
            self.writes.push(RouteChange::Upsert(replacement.clone()));
            if self.fail_after_write {
                return Err(CoreError::new(
                    ErrorKind::RouteFailure,
                    "injected route write failure",
                ));
            }
            Ok(())
        }

        fn remove_owned_route(&mut self, previous: &Route) -> Result<(), CoreError> {
            if !self.routes.contains(previous) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "kernel compare-and-swap route removal mismatch",
                ));
            }
            if !self.ignore_write {
                self.routes.retain(|route| route != previous);
            }
            self.writes.push(RouteChange::Remove(previous.clone()));
            if self.fail_after_write {
                return Err(CoreError::new(
                    ErrorKind::RouteFailure,
                    "injected route removal failure",
                ));
            }
            Ok(())
        }
    }

    #[test]
    fn changes_owned_routes_without_modifying_other_networks() {
        let foreign = route(2, 90, 91);
        let old = route(1, 3, 4);
        let obsolete = route(1, 5, 4);
        let desired = route(1, 3, 7);
        let mut fake = FakeRoutes::new(vec![foreign.clone(), old, obsolete.clone()]);
        let result =
            execute_route_reconciliation(network(1), std::slice::from_ref(&desired), &mut fake).unwrap();
        assert_eq!(
            result.applied,
            vec![
                RouteChange::Upsert(desired.clone()),
                RouteChange::Remove(obsolete)
            ]
        );
        assert_eq!(
            result.observed.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([foreign, desired])
        );
        assert!(
            execute_route_reconciliation(network(1), &[route(1, 3, 7)], &mut fake)
                .unwrap()
                .applied
                .is_empty()
        );
        assert_eq!(fake.writes.len(), 2);
    }

    #[test]
    fn foreign_collision_and_invalid_desired_do_not_write() {
        let mut fake = FakeRoutes::new(vec![route(2, 3, 9)]);
        assert_eq!(
            execute_route_reconciliation(network(1), &[route(1, 3, 7)], &mut fake)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
        assert!(fake.writes.is_empty());
        assert!(execute_route_reconciliation(network(0), &[], &mut fake).is_err());
        assert!(fake.writes.is_empty());
    }

    #[test]
    fn external_route_change_before_mutation_prevents_all_writes() {
        let foreign = route(2, 90, 91);
        let mut fake = FakeRoutes::new(vec![]);
        fake.mutate_on_observe = Some((2, foreign.clone()));
        let error =
            execute_route_reconciliation(network(1), &[route(1, 3, 7)], &mut fake).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(fake.routes, vec![foreign]);
        assert!(fake.writes.is_empty());
    }

    #[test]
    fn failure_after_partial_mutation_is_replanned_not_rolled_back() {
        let desired = route(1, 3, 7);
        let second = route(1, 4, 7);
        let mut fake = FakeRoutes::new(vec![]);
        fake.fail_after_write = true;
        assert!(
            execute_route_reconciliation(network(1), &[desired.clone(), second.clone()], &mut fake)
                .is_err()
        );
        assert_eq!(fake.routes, vec![desired.clone()]);
        fake.fail_after_write = false;
        let retry = execute_route_reconciliation(network(1), &[desired, second.clone()], &mut fake)
            .unwrap();
        assert_eq!(retry.applied, vec![RouteChange::Upsert(second)]);
    }

    #[test]
    fn success_without_kernel_effect_is_detected_and_stops() {
        let mut fake = FakeRoutes::new(vec![]);
        fake.ignore_write = true;
        let error =
            execute_route_reconciliation(network(1), &[route(1, 3, 7)], &mut fake).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::RouteFailure);
        assert_eq!(fake.writes.len(), 1);
    }

    #[test]
    fn concurrent_change_after_prior_operation_stops_before_next() {
        let foreign = route(2, 90, 91);
        let mut fake = FakeRoutes::new(vec![]);
        // Reads: 1 initial, 2 pre-first, 3 post-first, 4 pre-second.
        fake.mutate_on_observe = Some((4, foreign.clone()));
        let first = route(1, 3, 7);
        let second = route(1, 4, 7);
        assert_eq!(
            execute_route_reconciliation(network(1), &[first.clone(), second], &mut fake)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(fake.writes, vec![RouteChange::Upsert(first.clone())]);
        assert_eq!(
            fake.routes.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([first, foreign])
        );
    }
}

//! Software-only P9-005 bridge from confirmed PAN hub loss to topology execution.
//!
//! The caller must serialize this operation per network and supply a backend
//! whose observations are authoritative, whose peers/capabilities are already
//! authenticated, and whose link mutations enforce membership authorization.
//! This helper does not monitor BlueZ, authenticate peers, or prove RF recovery.
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CoreError, HubLossRecoveryPlan, HubLossTracker, NapAssignment, NapExecutionReport, NapLink,
    NapSelection, NapTopologyOperator, NodeId, TopologyChangeDecision, TopologyChangeObservation,
    TopologyPolicyInputs, TopologyStabilityPolicy, decide_topology_change, execute_nap_plan,
    plan_hub_loss_recovery,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubLossRecoveryExecution {
    pub confirmed_lost: BTreeSet<NodeId>,
    pub plan: HubLossRecoveryPlan,
    pub execution: NapExecutionReport,
}

fn observed_selection(observed: &BTreeSet<NapLink>) -> NapSelection {
    let mut stars: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for link in observed {
        stars.entry(link.hub).or_default().push(link.client);
    }
    NapSelection {
        stars: stars
            .into_iter()
            .map(|(hub, clients)| NapAssignment { hub, clients })
            .collect(),
        unserved: BTreeSet::new(),
    }
}

/// Retry state for confirmed hub-loss recovery. This controls repeated software
/// recovery attempts, not the hub-loss confirmation interval or radio probing.
/// The daemon must retain one instance per network and serialize its callbacks.
#[derive(Clone, Debug)]
pub struct HubRecoveryRetryState {
    policy: TopologyStabilityPolicy,
    consecutive_failures: u32,
    last_failure_ms: Option<u64>,
}

impl HubRecoveryRetryState {
    pub fn new(policy: TopologyStabilityPolicy) -> Result<Self, CoreError> {
        let state = Self {
            policy,
            consecutive_failures: 0,
            last_failure_ms: None,
        };
        // Validate the policy before accepting it, even when no loss exists.
        state.is_due(0)?;
        Ok(state)
    }

    pub const fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub const fn last_failure_ms(&self) -> Option<u64> {
        self.last_failure_ms
    }

    fn is_due(&self, now_ms: u64) -> Result<bool, CoreError> {
        // A confirmed missing hub is not a viable current path: bypass normal
        // hysteresis/dwell, but still enforce bounded failed-attempt backoff.
        let decision = decide_topology_change(
            self.policy,
            TopologyChangeObservation {
                now_ms,
                last_change_ms: 0,
                current_viable: false,
                proposed_viable: true,
                current_cost: 0,
                proposed_cost: 0,
                consecutive_failures: self.consecutive_failures,
                last_failure_ms: self.last_failure_ms,
            },
        )?;
        Ok(matches!(decision, TopologyChangeDecision::Apply))
    }

    fn failed(&mut self, now_ms: u64) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_failure_ms = Some(now_ms);
    }

    fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure_ms = None;
    }
}

/// Sample hub loss on every call, even while a previous failed recovery is in
/// backoff. Deferred retries make no PAN-link or route mutations. Once due,
/// the same strict observation checks and operation ordering as the ungated
/// recovery path apply. An error records a bounded next-attempt delay; success
/// or a fully restored hub clears the failure history.
///
/// This is an opt-in software seam for future authenticated daemon ownership,
/// not a live BlueZ worker or a promise of eventual recovery.
pub fn execute_confirmed_hub_loss_recovery_with_backoff(
    inputs: &TopologyPolicyInputs,
    tracker: &mut HubLossTracker,
    retry: &mut HubRecoveryRetryState,
    now_ms: u64,
    operator: &mut impl NapTopologyOperator,
) -> Result<Option<HubLossRecoveryExecution>, CoreError> {
    let observed = operator.observe_links()?;
    let lost = tracker.observe(now_ms, &observed)?;
    if lost.is_empty() {
        retry.reset();
        return Ok(None);
    }
    if !retry.is_due(now_ms)? {
        return Ok(None);
    }
    match recover_from_observation(inputs, &observed, lost, operator) {
        Ok(result) => {
            retry.reset();
            Ok(Some(result))
        }
        Err(error) => {
            retry.failed(now_ms);
            Err(error)
        }
    }
}

/// Observe the backend, debounce hub loss, and attempt a fresh recovery plan.
///
/// No link changes occur before loss is confirmed. On confirmed loss, stale
/// routes are reconciled against *observed* surviving links before replacement
/// connections are attempted. The executor observes again before mutating
/// links and after each step; any failure stops the attempt. Call again with a
/// later timestamp to replan from the actual backend state, never from the
/// previous request's presumed success.
pub fn execute_confirmed_hub_loss_recovery(
    inputs: &TopologyPolicyInputs,
    tracker: &mut HubLossTracker,
    now_ms: u64,
    operator: &mut impl NapTopologyOperator,
) -> Result<Option<HubLossRecoveryExecution>, CoreError> {
    let observed = operator.observe_links()?;
    let confirmed_lost = tracker.observe(now_ms, &observed)?;
    if confirmed_lost.is_empty() {
        return Ok(None);
    }

    recover_from_observation(inputs, &observed, confirmed_lost, operator).map(Some)
}

/// A confirmed loss must only be executed against the observation it was
/// confirmed with. Both public entry points use this same fail-closed path.
fn recover_from_observation(
    inputs: &TopologyPolicyInputs,
    observed: &BTreeSet<NapLink>,
    confirmed_lost: BTreeSet<NodeId>,
    operator: &mut impl NapTopologyOperator,
) -> Result<HubLossRecoveryExecution, CoreError> {
    // Validate observations and policy before touching routes or links.
    let plan = plan_hub_loss_recovery(inputs, observed, &confirmed_lost)?;
    // Planning and the first observation are not an atomic PAN snapshot.
    // A link can appear or disappear while selecting the replacement hubs.
    // Never reconcile routes against an already-obsolete observation.
    let current = operator.observe_links()?;
    if &current != observed {
        return Err(CoreError::new(
            crate::ErrorKind::InvalidState,
            "PAN observations changed before route reconciliation; replan",
        ));
    }
    operator.reconcile_routes(&current)?;
    let active = observed_selection(&current);
    let execution = execute_nap_plan(inputs, &active, &plan.desired, operator)?;
    Ok(HubLossRecoveryExecution {
        confirmed_lost,
        plan,
        execution,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CandidateLink, CapabilitySource, ErrorKind, NapPlanStep, NodeCapabilities, Sourced,
        TopologyCandidate,
    };

    fn id(n: u8) -> NodeId {
        NodeId::from_bytes([n; 16])
    }

    fn link(hub: u8, client: u8) -> NapLink {
        NapLink {
            hub: id(hub),
            client: id(client),
        }
    }

    fn policy() -> TopologyPolicyInputs {
        let mut candidates = BTreeMap::new();
        for n in 1..=4 {
            candidates.insert(
                id(n),
                TopologyCandidate {
                    id: id(n),
                    capabilities: NodeCapabilities {
                        adapter_usable: Some(Sourced::new(true, CapabilitySource::Measured)),
                        nap: Some(Sourced::new(n <= 2, CapabilitySource::Measured)),
                        panu: Some(Sourced::new(n >= 3, CapabilitySource::Measured)),
                        connection_policy_ceiling: Some(Sourced::new(
                            2,
                            CapabilitySource::Measured,
                        )),
                        ..NodeCapabilities::default()
                    },
                    neighbors: BTreeMap::new(),
                    currently_hosting: false,
                    gateway_preferred: false,
                },
            );
        }
        for hub in [1, 2] {
            for client in [3, 4] {
                let candidate = CandidateLink {
                    path_cost: 1,
                    quality: None,
                    stable: true,
                };
                candidates
                    .get_mut(&id(hub))
                    .unwrap()
                    .neighbors
                    .insert(id(client), candidate);
                candidates
                    .get_mut(&id(client))
                    .unwrap()
                    .neighbors
                    .insert(id(hub), candidate);
            }
        }
        TopologyPolicyInputs { candidates }
    }

    struct FakePan {
        links: BTreeSet<NapLink>,
        routes: BTreeSet<NapLink>,
        operations: Vec<NapPlanStep>,
        fail_connect: Option<NapLink>,
        observe_count: usize,
        observe_change: Option<(usize, BTreeSet<NapLink>)>,
    }

    impl FakePan {
        fn hosted() -> Self {
            let links = BTreeSet::from([link(1, 3), link(1, 4)]);
            Self {
                routes: links.clone(),
                links,
                operations: Vec::new(),
                fail_connect: None,
                observe_count: 0,
                observe_change: None,
            }
        }
    }

    impl NapTopologyOperator for FakePan {
        fn observe_links(&mut self) -> Result<BTreeSet<NapLink>, CoreError> {
            self.observe_count += 1;
            if self
                .observe_change
                .as_ref()
                .is_some_and(|(at, _)| *at == self.observe_count)
            {
                self.links = self.observe_change.take().unwrap().1;
            }
            Ok(self.links.clone())
        }

        fn connect_link(&mut self, new: NapLink) -> Result<(), CoreError> {
            if self.fail_connect == Some(new) {
                return Err(CoreError::new(
                    ErrorKind::TopologyFailure,
                    "injected PAN connection failure",
                ));
            }
            self.links.insert(new);
            self.operations.push(NapPlanStep::Connect(new));
            Ok(())
        }

        fn disconnect_link(&mut self, old: NapLink) -> Result<(), CoreError> {
            self.links.remove(&old);
            self.operations.push(NapPlanStep::Disconnect(old));
            Ok(())
        }

        fn reconcile_routes(&mut self, observed: &BTreeSet<NapLink>) -> Result<(), CoreError> {
            self.routes = observed.clone();
            Ok(())
        }
    }

    #[test]
    fn debounced_loss_reconnects_only_after_confirmation_and_repairs_routes() {
        let inputs = policy();
        let mut tracker = HubLossTracker::new(10).unwrap();
        let mut pan = FakePan::hosted();
        assert!(
            execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 0, &mut pan)
                .unwrap()
                .is_none()
        );

        pan.links.clear(); // Independent simulated PAN observation, not a requested disconnect.
        for now in [1, 10] {
            assert!(
                execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, now, &mut pan)
                    .unwrap()
                    .is_none()
            );
        }
        assert!(pan.operations.is_empty());
        let result = execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 11, &mut pan)
            .unwrap()
            .unwrap();
        let recovered = BTreeSet::from([link(2, 3), link(2, 4)]);
        assert_eq!(result.confirmed_lost, BTreeSet::from([id(1)]));
        assert_eq!(result.plan.unavailable, BTreeSet::from([id(1)]));
        assert_eq!(result.execution.observed, recovered);
        assert_eq!(pan.routes, recovered);
        assert_eq!(
            pan.operations,
            vec![
                NapPlanStep::Connect(link(2, 3)),
                NapPlanStep::Connect(link(2, 4)),
            ]
        );

        // Repeated confirmed-loss notifications must not create duplicate links.
        let repeat = execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 12, &mut pan)
            .unwrap()
            .unwrap();
        assert!(repeat.execution.applied.is_empty());
        assert_eq!(pan.operations.len(), 2);
    }

    #[test]
    fn failed_reconnect_replans_from_partial_observed_state() {
        let inputs = policy();
        let mut tracker = HubLossTracker::new(10).unwrap();
        let mut pan = FakePan::hosted();
        execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 0, &mut pan).unwrap();
        pan.links.clear();
        execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 1, &mut pan).unwrap();
        pan.fail_connect = Some(link(2, 4));
        assert!(execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 11, &mut pan).is_err());
        assert_eq!(pan.links, BTreeSet::from([link(2, 3)]));
        assert_eq!(pan.routes, pan.links);

        pan.fail_connect = None;
        let retry = execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 12, &mut pan)
            .unwrap()
            .unwrap();
        assert_eq!(
            retry.execution.applied,
            vec![NapPlanStep::Connect(link(2, 4))]
        );
        assert_eq!(pan.routes, BTreeSet::from([link(2, 3), link(2, 4)]));
        assert_eq!(pan.operations.len(), 2);
    }

    #[test]
    fn changed_pan_snapshot_blocks_stale_route_reconciliation_and_replans() {
        let inputs = policy();
        let mut tracker = HubLossTracker::new(10).unwrap();
        let mut pan = FakePan::hosted();
        execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 0, &mut pan).unwrap();
        pan.links.clear();
        execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 1, &mut pan).unwrap();

        let stale_routes = pan.routes.clone();
        // A replacement PAN link appears after planning but before routes
        // would be reconciled. No link mutation or stale route write is safe.
        pan.observe_change = Some((pan.observe_count + 2, BTreeSet::from([link(2, 3)])));
        let error =
            execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 11, &mut pan).unwrap_err();
        assert!(error.to_string().contains("PAN observations changed"));
        assert_eq!(pan.routes, stale_routes);
        assert!(pan.operations.is_empty());
        assert_eq!(pan.links, BTreeSet::from([link(2, 3)]));

        // A later observation must repair routes and complete the missing
        // connection from the actual PAN state, not the obsolete snapshot.
        let retry = execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 12, &mut pan)
            .unwrap()
            .unwrap();
        assert_eq!(
            retry.execution.applied,
            vec![NapPlanStep::Connect(link(2, 4))]
        );
        assert_eq!(pan.routes, BTreeSet::from([link(2, 3), link(2, 4)]));
    }

    fn retry_policy() -> TopologyStabilityPolicy {
        TopologyStabilityPolicy {
            min_dwell_ms: 1_000,
            min_cost_improvement: 1,
            retry_base_ms: 100,
            retry_max_ms: 400,
        }
    }

    #[test]
    fn hub_loss_retry_backoff_defers_mutations_but_keeps_sampling() {
        let inputs = policy();
        let mut tracker = HubLossTracker::new(10).unwrap();
        let mut retry = HubRecoveryRetryState::new(retry_policy()).unwrap();
        let mut pan = FakePan::hosted();
        execute_confirmed_hub_loss_recovery_with_backoff(
            &inputs,
            &mut tracker,
            &mut retry,
            0,
            &mut pan,
        )
        .unwrap();
        pan.links.clear();
        execute_confirmed_hub_loss_recovery_with_backoff(
            &inputs,
            &mut tracker,
            &mut retry,
            1,
            &mut pan,
        )
        .unwrap();
        pan.fail_connect = Some(link(2, 4));
        assert!(
            execute_confirmed_hub_loss_recovery_with_backoff(
                &inputs,
                &mut tracker,
                &mut retry,
                11,
                &mut pan,
            )
            .is_err()
        );
        assert_eq!(retry.consecutive_failures(), 1);
        assert_eq!(retry.last_failure_ms(), Some(11));
        let routes_before = pan.routes.clone();
        let operations_before = pan.operations.clone();
        let observations_before = pan.observe_count;
        pan.fail_connect = None;
        assert!(
            execute_confirmed_hub_loss_recovery_with_backoff(
                &inputs,
                &mut tracker,
                &mut retry,
                110,
                &mut pan,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(pan.observe_count, observations_before + 1);
        assert_eq!(pan.routes, routes_before);
        assert_eq!(pan.operations, operations_before);
        let result = execute_confirmed_hub_loss_recovery_with_backoff(
            &inputs,
            &mut tracker,
            &mut retry,
            111,
            &mut pan,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.execution.applied,
            vec![NapPlanStep::Connect(link(2, 4))]
        );
        assert_eq!(pan.routes, BTreeSet::from([link(2, 3), link(2, 4)]));
        assert_eq!(retry.consecutive_failures(), 0);
        assert_eq!(retry.last_failure_ms(), None);
    }

    #[test]
    fn restored_hub_clears_retry_history_and_invalid_clock_cannot_retry() {
        let inputs = policy();
        let mut tracker = HubLossTracker::new(10).unwrap();
        let mut retry = HubRecoveryRetryState::new(retry_policy()).unwrap();
        let mut pan = FakePan::hosted();
        execute_confirmed_hub_loss_recovery_with_backoff(
            &inputs,
            &mut tracker,
            &mut retry,
            0,
            &mut pan,
        )
        .unwrap();
        pan.links.clear();
        execute_confirmed_hub_loss_recovery_with_backoff(
            &inputs,
            &mut tracker,
            &mut retry,
            1,
            &mut pan,
        )
        .unwrap();
        pan.fail_connect = Some(link(2, 3));
        assert!(
            execute_confirmed_hub_loss_recovery_with_backoff(
                &inputs,
                &mut tracker,
                &mut retry,
                11,
                &mut pan,
            )
            .is_err()
        );
        assert_eq!(retry.consecutive_failures(), 1);
        assert!(
            execute_confirmed_hub_loss_recovery_with_backoff(
                &inputs,
                &mut tracker,
                &mut retry,
                11,
                &mut pan,
            )
            .is_err()
        );
        assert_eq!(retry.consecutive_failures(), 1);

        // The original host reappears; no future loss should inherit old backoff.
        pan.links = BTreeSet::from([link(1, 3), link(1, 4)]);
        assert!(
            execute_confirmed_hub_loss_recovery_with_backoff(
                &inputs,
                &mut tracker,
                &mut retry,
                12,
                &mut pan,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(retry.consecutive_failures(), 0);
        pan.links.clear();
        execute_confirmed_hub_loss_recovery_with_backoff(
            &inputs,
            &mut tracker,
            &mut retry,
            13,
            &mut pan,
        )
        .unwrap();
        pan.fail_connect = None;
        assert!(
            execute_confirmed_hub_loss_recovery_with_backoff(
                &inputs,
                &mut tracker,
                &mut retry,
                23,
                &mut pan,
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn invalid_retry_policy_is_rejected_before_observation() {
        let mut bad = retry_policy();
        bad.retry_base_ms = 0;
        assert_eq!(
            HubRecoveryRetryState::new(bad).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn intentional_removal_does_not_trigger_recovery() {
        let inputs = policy();
        let mut tracker = HubLossTracker::new(5).unwrap();
        let mut pan = FakePan::hosted();
        execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 0, &mut pan).unwrap();
        tracker.forget_hub(id(1));
        pan.links.clear();
        assert!(
            execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 1, &mut pan)
                .unwrap()
                .is_none()
        );
        assert!(
            execute_confirmed_hub_loss_recovery(&inputs, &mut tracker, 10, &mut pan)
                .unwrap()
                .is_none()
        );
        assert!(pan.operations.is_empty());
    }
}

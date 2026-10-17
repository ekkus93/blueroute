//! Software-only P9-005 bridge from confirmed PAN hub loss to topology execution.
//!
//! The caller must serialize this operation per network and supply a backend
//! whose observations are authoritative, whose peers/capabilities are already
//! authenticated, and whose link mutations enforce membership authorization.
//! This helper does not monitor BlueZ, authenticate peers, or prove RF recovery.
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CoreError, HubLossRecoveryPlan, HubLossTracker, NapAssignment, NapExecutionReport, NapLink,
    NapSelection, NapTopologyOperator, NodeId, TopologyPolicyInputs, execute_nap_plan,
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

    // Validate observations and policy before touching routes or links.
    let plan = plan_hub_loss_recovery(inputs, &observed, &confirmed_lost)?;
    operator.reconcile_routes(&observed)?;
    let active = observed_selection(&observed);
    let execution = execute_nap_plan(inputs, &active, &plan.desired, operator)?;
    Ok(Some(HubLossRecoveryExecution {
        confirmed_lost,
        plan,
        execution,
    }))
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
    }

    impl FakePan {
        fn hosted() -> Self {
            let links = BTreeSet::from([link(1, 3), link(1, 4)]);
            Self {
                routes: links.clone(),
                links,
                operations: Vec::new(),
                fail_connect: None,
            }
        }
    }

    impl NapTopologyOperator for FakePan {
        fn observe_links(&mut self) -> Result<BTreeSet<NapLink>, CoreError> {
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

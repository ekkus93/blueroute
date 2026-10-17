//! P9-005 deterministic hub-loss recovery proposal.
//!
//! A caller must independently confirm link loss and supply the *observed*
//! surviving links. This module cannot detect radio loss, authorize remote
//! capability claims, reconnect BlueZ PANs, or install routes.
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CoreError, ErrorKind, NapAssignment, NapLink, NapPlanDiff, NapSelection, NodeId,
    TopologyPolicyInputs, diff_nap_topology, select_nap_hubs,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubLossRecoveryPlan {
    /// Proposed replacement links; no connection is implied by this value.
    pub desired: NapSelection,
    /// Ordered intents relative to the independently observed surviving links.
    pub transition: NapPlanDiff,
    /// Failed nodes and survivors for which no capable path was found.
    pub unavailable: BTreeSet<NodeId>,
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidInput, message)
}

/// Propose recovery after independently observed NAP loss.
///
/// The lost hubs must be known nodes and absent from all currently observed
/// links. Inconsistent or stale observations fail closed rather than
/// manufacturing a recovery result. Existing healthy links are passed to the
/// normal minimal-diff planner, so unchanged links are never disconnected by
/// the transition. The caller must reconcile routes against observed links
/// before executing this proposal and after every successful backend change.
pub fn plan_hub_loss_recovery(
    inputs: &TopologyPolicyInputs,
    observed_links: &BTreeSet<NapLink>,
    lost_hubs: &BTreeSet<NodeId>,
) -> Result<HubLossRecoveryPlan, CoreError> {
    inputs.validate()?;
    if lost_hubs.is_empty()
        || !lost_hubs.is_subset(&inputs.candidates.keys().copied().collect())
    {
        return Err(invalid("hub loss requires known failed node identities"));
    }

    let mut active_stars: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    let mut attached_clients = BTreeSet::new();
    for link in observed_links {
        if link.hub == link.client
            || !inputs.candidates.contains_key(&link.hub)
            || !inputs.candidates.contains_key(&link.client)
            || lost_hubs.contains(&link.hub)
            || lost_hubs.contains(&link.client)
            || !attached_clients.insert(link.client)
        {
            return Err(invalid("stale, unknown or conflicting PAN observations"));
        }
        active_stars.entry(link.hub).or_default().push(link.client);
    }
    let active = NapSelection {
        stars: active_stars
            .into_iter()
            .map(|(hub, clients)| NapAssignment { hub, clients })
            .collect(),
        unserved: BTreeSet::new(),
    };

    // Exclude failed nodes from selection and remove every edge to them.
    // Do not infer a replacement radio path from a previously failed hub.
    let mut available = inputs.clone();
    available.candidates.retain(|id, _| !lost_hubs.contains(id));
    for (id, candidate) in &mut available.candidates {
        candidate.neighbors.retain(|peer, _| !lost_hubs.contains(peer));
        candidate.currently_hosting = observed_links.iter().any(|link| link.hub == *id);
    }
    let mut desired = select_nap_hubs(&available)?;
    desired.unserved.extend(lost_hubs.iter().copied());
    let transition = diff_nap_topology(inputs, &active, &desired)?;
    let unavailable = desired.unserved.clone();
    Ok(HubLossRecoveryPlan {
        desired,
        transition,
        unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CandidateLink, CapabilitySource, NapPlanStep, NodeCapabilities, Sourced,
        TopologyCandidate,
    };

    fn id(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn link(hub: u8, client: u8) -> NapLink {
        NapLink {
            hub: id(hub),
            client: id(client),
        }
    }

    fn fixture(with_alternate: bool) -> TopologyPolicyInputs {
        let mut candidates = BTreeMap::new();
        for n in 1..=4 {
            candidates.insert(
                id(n),
                TopologyCandidate {
                    id: id(n),
                    capabilities: NodeCapabilities {
                        adapter_usable: Some(Sourced::new(true, CapabilitySource::Measured)),
                        nap: Some(Sourced::new(
                            n == 1 || (n == 2 && with_alternate),
                            CapabilitySource::Measured,
                        )),
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
                let observation = CandidateLink {
                    path_cost: 1,
                    quality: None,
                    stable: true,
                };
                candidates
                    .get_mut(&id(hub))
                    .unwrap()
                    .neighbors
                    .insert(id(client), observation);
                candidates
                    .get_mut(&id(client))
                    .unwrap()
                    .neighbors
                    .insert(id(hub), observation);
            }
        }
        TopologyPolicyInputs { candidates }
    }

    #[test]
    fn alternate_capable_hub_restores_two_disconnected_clients() {
        let plan = plan_hub_loss_recovery(
            &fixture(true),
            &BTreeSet::new(),
            &BTreeSet::from([id(1)]),
        )
        .unwrap();
        assert_eq!(plan.desired.stars.len(), 1);
        assert_eq!(plan.desired.stars[0].hub, id(2));
        assert_eq!(plan.desired.stars[0].clients, vec![id(3), id(4)]);
        assert_eq!(
            plan.transition.steps,
            vec![
                NapPlanStep::Connect(link(2, 3)),
                NapPlanStep::Connect(link(2, 4)),
            ]
        );
        assert_eq!(plan.unavailable, BTreeSet::from([id(1)]));
    }

    #[test]
    fn no_capable_alternative_reports_unserved_without_fabricated_links() {
        let plan = plan_hub_loss_recovery(
            &fixture(false),
            &BTreeSet::new(),
            &BTreeSet::from([id(1)]),
        )
        .unwrap();
        assert!(plan.desired.stars.is_empty());
        assert!(plan.transition.steps.is_empty());
        assert_eq!(plan.unavailable, BTreeSet::from([id(1), id(2), id(3), id(4)]));
    }

    #[test]
    fn healthy_surviving_link_is_retained_during_recovery() {
        let plan = plan_hub_loss_recovery(
            &fixture(true),
            &BTreeSet::from([link(2, 3)]),
            &BTreeSet::from([id(1)]),
        )
        .unwrap();
        assert_eq!(
            plan.transition.steps,
            vec![NapPlanStep::Connect(link(2, 4))]
        );
        assert!(plan.transition.reconnect_interruptions.is_empty());
    }

    #[test]
    fn stale_lost_hub_link_and_unknown_loss_fail_closed() {
        let inputs = fixture(true);
        assert!(plan_hub_loss_recovery(
            &inputs,
            &BTreeSet::from([link(1, 3)]),
            &BTreeSet::from([id(1)]),
        )
        .is_err());
        assert!(plan_hub_loss_recovery(
            &inputs,
            &BTreeSet::new(),
            &BTreeSet::from([id(9)]),
        )
        .is_err());
        assert!(plan_hub_loss_recovery(&inputs, &BTreeSet::new(), &BTreeSet::new()).is_err());
    }

    #[test]
    fn conflicting_surviving_clients_fail_closed() {
        let inputs = fixture(true);
        assert!(plan_hub_loss_recovery(
            &inputs,
            &BTreeSet::from([link(1, 3), link(2, 3)]),
            &BTreeSet::from([id(4)]),
        )
        .is_err());
    }
}

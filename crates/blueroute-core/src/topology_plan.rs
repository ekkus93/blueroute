//! Pure, conservative P9-003 NAP topology reconciliation.
//!
//! This module plans changes only. It never authorizes peers, operates BlueZ,
//! establishes routes, or assumes that a proposed link is already connected.
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    CoreError, ErrorKind, NapSelection, NodeId, TopologyPolicyInputs,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NapLink {
    pub hub: NodeId,
    pub client: NodeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NapPlanStep {
    Connect(NapLink),
    Disconnect(NapLink),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NapPlanDiff {
    /// Ordered, idempotently reconcilable intents, not observed link success.
    pub steps: Vec<NapPlanStep>,
    /// Clients whose migration cannot be make-before-break under a
    /// conservative single-PANU-attachment policy.
    pub reconnect_interruptions: BTreeSet<NodeId>,
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidInput, message)
}

fn link_set(
    plan: &NapSelection,
    inputs: &TopologyPolicyInputs,
    desired: bool,
) -> Result<BTreeSet<NapLink>, CoreError> {
    let mut links = BTreeSet::new();
    let mut hubs = BTreeSet::new();
    let mut client_hubs = BTreeMap::new();
    let mut served = BTreeSet::new();
    for star in &plan.stars {
        if !hubs.insert(star.hub) || star.clients.is_empty() {
            return Err(invalid("duplicate or empty NAP assignment"));
        }
        if desired {
            let hub = inputs
                .candidates
                .get(&star.hub)
                .ok_or_else(|| invalid("unknown desired NAP hub"))?;
            if hub.capabilities.can_host_pan() != Some(true)
                || hub.capabilities.adapter_usable.as_ref().map(|v| v.value) != Some(true)
            {
                return Err(invalid("desired NAP host lacks affirmative capability"));
            }
            let capacity = hub
                .capabilities
                .connection_policy_ceiling
                .as_ref()
                .map(|v| usize::from(v.value))
                .ok_or_else(|| invalid("desired NAP host capacity is unknown"))?;
            if star.clients.len() > capacity {
                return Err(invalid("desired NAP host exceeds capacity"));
            }
        }
        served.insert(star.hub);
        for client in &star.clients {
            if *client == star.hub || client_hubs.insert(*client, star.hub).is_some() {
                return Err(invalid("self-link or multiply assigned PANU client"));
            }
            if desired {
                let host = &inputs.candidates[&star.hub];
                let peer = inputs
                    .candidates
                    .get(client)
                    .ok_or_else(|| invalid("unknown desired PANU client"))?;
                if peer.capabilities.can_join_pan() != Some(true)
                    || peer.capabilities.adapter_usable.as_ref().map(|v| v.value) != Some(true)
                    || !host.neighbors.contains_key(client)
                    || !peer.neighbors.contains_key(&star.hub)
                {
                    return Err(invalid("desired link lacks reciprocal, capable neighbors"));
                }
            }
            served.insert(*client);
            links.insert(NapLink {
                hub: star.hub,
                client: *client,
            });
        }
    }
    if desired {
        let known: BTreeSet<_> = inputs.candidates.keys().copied().collect();
        if !plan.unserved.is_disjoint(&served)
            || !plan.unserved.is_subset(&known)
            || served.union(&plan.unserved).copied().collect::<BTreeSet<_>>() != known
        {
            return Err(invalid("desired topology has inconsistent unserved nodes"));
        }
    }
    Ok(links)
}

/// Compute an ordered, minimal diff between observed and desired NAP links.
///
/// Unchanged links are never torn down. New independent clients are connected
/// before obsolete links are retired, subject to measured hub capacity.
/// A client moving hubs is disconnected first: the policy does not presume
/// simultaneous PANU attachments. Capacity conflicts also require retiring an
/// obsolete link before its replacement. Execution must observe each step and
/// re-plan on failure; this function does not claim any link succeeded.
pub fn diff_nap_topology(
    inputs: &TopologyPolicyInputs,
    active: &NapSelection,
    desired: &NapSelection,
) -> Result<NapPlanDiff, CoreError> {
    inputs.validate()?;
    let mut current = link_set(active, inputs, false)?;
    let target = link_set(desired, inputs, true)?;
    let mut pending: BTreeSet<_> = target.difference(&current).copied().collect();
    let mut result = NapPlanDiff::default();

    while !pending.is_empty() {
        // Prefer make-before-break wherever no active client or hub capacity
        // constraint prevents it.
        let ready = pending.iter().copied().find(|link| {
            let attached = current.iter().any(|old| old.client == link.client);
            let capacity = inputs.candidates[&link.hub]
                .capabilities
                .connection_policy_ceiling
                .as_ref()
                .map(|v| usize::from(v.value))
                .unwrap_or(0);
            let occupied = current.iter().filter(|old| old.hub == link.hub).count();
            !attached && occupied < capacity
        });
        if let Some(link) = ready {
            pending.remove(&link);
            current.insert(link);
            result.steps.push(NapPlanStep::Connect(link));
            continue;
        }

        // Every blocked addition must have an obsolete attachment to free:
        // either the moving client's old hub or an obsolete slot on its new
        // hub. Never remove a link retained by the desired topology.
        let blocked = pending.iter().copied().find_map(|link| {
            current
                .difference(&target)
                .copied()
                .find(|old| old.client == link.client)
                .or_else(|| {
                    current
                        .difference(&target)
                        .copied()
                        .find(|old| old.hub == link.hub)
                })
        });
        let old = blocked.ok_or_else(|| invalid("topology transition cannot free a safe slot"))?;
        current.remove(&old);
        if target.iter().any(|new| new.client == old.client) {
            result.reconnect_interruptions.insert(old.client);
        }
        result.steps.push(NapPlanStep::Disconnect(old));
    }
    for old in current.difference(&target) {
        result.steps.push(NapPlanStep::Disconnect(*old));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CandidateLink, CapabilitySource, NapAssignment, NodeCapabilities, Sourced,
        TopologyCandidate,
    };

    fn id(n: u8) -> NodeId {
        NodeId::from_bytes([n; 16])
    }

    fn inputs() -> TopologyPolicyInputs {
        let mut candidates = BTreeMap::new();
        for n in 1..=5 {
            let mut neighbors = BTreeMap::new();
            for other in 1..=5 {
                if n != other {
                    neighbors.insert(
                        id(other),
                        CandidateLink {
                            path_cost: 1,
                            quality: None,
                            stable: true,
                        },
                    );
                }
            }
            candidates.insert(
                id(n),
                TopologyCandidate {
                    id: id(n),
                    capabilities: NodeCapabilities {
                        nap: Some(Sourced::new(n <= 2, CapabilitySource::Measured)),
                        panu: Some(Sourced::new(true, CapabilitySource::Measured)),
                        adapter_usable: Some(Sourced::new(true, CapabilitySource::Measured)),
                        connection_policy_ceiling: Some(Sourced::new(2, CapabilitySource::Measured)),
                        ..NodeCapabilities::default()
                    },
                    neighbors,
                    currently_hosting: false,
                    gateway_preferred: false,
                },
            );
        }
        TopologyPolicyInputs { candidates }
    }

    fn plan(stars: &[(u8, &[u8])]) -> NapSelection {
        let assignments: Vec<_> = stars
            .iter()
            .map(|(hub, clients)| NapAssignment {
                hub: id(*hub),
                clients: clients.iter().map(|n| id(*n)).collect(),
            })
            .collect();
        let served: BTreeSet<_> = assignments
            .iter()
            .flat_map(|star| std::iter::once(star.hub).chain(star.clients.iter().copied()))
            .collect();
        NapSelection {
            stars: assignments,
            unserved: (1..=5).map(id).filter(|node| !served.contains(node)).collect(),
        }
    }

    fn link(hub: u8, client: u8) -> NapLink {
        NapLink {
            hub: id(hub),
            client: id(client),
        }
    }

    #[test]
    fn stable_plan_and_metric_only_changes_have_no_operations() {
        let mut policy = inputs();
        let active = plan(&[(1, &[3, 4])]);
        assert!(diff_nap_topology(&policy, &active, &active).unwrap().steps.is_empty());
        policy.candidates.get_mut(&id(1)).unwrap().neighbors.get_mut(&id(3)).unwrap().path_cost = 50;
        assert!(diff_nap_topology(&policy, &active, &active).unwrap().steps.is_empty());
    }

    #[test]
    fn independent_replacement_is_connected_before_obsolete_link_is_removed() {
        let result = diff_nap_topology(
            &inputs(),
            &plan(&[(1, &[3])]),
            &plan(&[(1, &[4])]),
        )
        .unwrap();
        assert_eq!(
            result.steps,
            vec![
                NapPlanStep::Connect(link(1, 4)),
                NapPlanStep::Disconnect(link(1, 3)),
            ]
        );
        assert!(result.reconnect_interruptions.is_empty());
    }

    #[test]
    fn moving_client_is_break_before_make_and_explicitly_reported() {
        let result = diff_nap_topology(
            &inputs(),
            &plan(&[(1, &[3])]),
            &plan(&[(2, &[3])]),
        )
        .unwrap();
        assert_eq!(
            result.steps,
            vec![
                NapPlanStep::Disconnect(link(1, 3)),
                NapPlanStep::Connect(link(2, 3)),
            ]
        );
        assert_eq!(result.reconnect_interruptions, BTreeSet::from([id(3)]));
    }

    #[test]
    fn full_hub_releases_only_obsolete_slot_before_connecting() {
        let result = diff_nap_topology(
            &inputs(),
            &plan(&[(1, &[3, 4])]),
            &plan(&[(1, &[3, 5])]),
        )
        .unwrap();
        assert_eq!(
            result.steps,
            vec![
                NapPlanStep::Disconnect(link(1, 4)),
                NapPlanStep::Connect(link(1, 5)),
            ]
        );
    }

    #[test]
    fn rejects_overcapacity_unknown_and_duplicate_desired_assignments() {
        let policy = inputs();
        assert!(diff_nap_topology(&policy, &plan(&[]), &plan(&[(1, &[3, 4, 5])])).is_err());
        assert!(diff_nap_topology(&policy, &plan(&[]), &plan(&[(3, &[4])])).is_err());
        assert!(diff_nap_topology(&policy, &plan(&[]), &plan(&[(1, &[3]), (2, &[3])])).is_err());
    }
}

//! Explicit, hardware-independent inputs for future P9 topology selection.
//! Discovery claims must be authenticated before becoming policy inputs.
use std::collections::BTreeMap;

use crate::{CoreError, ErrorKind, LinkQuality, NodeCapabilities, NodeId, PowerState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CandidateLink {
    pub path_cost: u32,
    pub quality: Option<LinkQuality>,
    pub stable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopologyCandidate {
    pub id: NodeId,
    pub capabilities: NodeCapabilities,
    pub neighbors: BTreeMap<NodeId, CandidateLink>,
    pub currently_hosting: bool,
    pub gateway_preferred: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TopologyPolicyInputs {
    pub candidates: BTreeMap<NodeId, TopologyCandidate>,
}

impl TopologyPolicyInputs {
    /// Validate before planning. Unknown capability is never affirmative
    /// permission to host or join; validation does not confer trust.
    pub fn validate(&self) -> Result<(), CoreError> {
        for (id, candidate) in &self.candidates {
            if *id == NodeId::from_bytes([0; 16]) || *id != candidate.id {
                return Err(invalid("invalid or mismatched topology node identity"));
            }
            if candidate
                .capabilities
                .connection_policy_ceiling
                .as_ref()
                .is_some_and(|ceiling| ceiling.value == 0)
            {
                return Err(invalid("zero NAP connection ceiling"));
            }
            if candidate.capabilities.power_state.as_ref().is_some_and(
                |power| matches!(power.value, PowerState::Battery { percent } if percent > 100),
            ) {
                return Err(invalid("battery percentage exceeds 100"));
            }
            for (neighbor, link) in &candidate.neighbors {
                if neighbor == id || !self.candidates.contains_key(neighbor) {
                    return Err(invalid("unknown or self-referential topology neighbor"));
                }
                if link.path_cost == 0 || link.quality.is_some_and(|quality| quality.score > 100) {
                    return Err(invalid("invalid topology link metric"));
                }
            }
        }
        Ok(())
    }
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidInput, message)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NapAssignment {
    pub hub: NodeId,
    pub clients: Vec<NodeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NapSelection {
    pub stars: Vec<NapAssignment>,
    /// Unreachable/unassigned candidates are explicitly retained, never
    /// silently presented as connected or authenticated.
    pub unserved: std::collections::BTreeSet<NodeId>,
}

/// Deterministic, capacity-bounded P9-002 NAP proposal. This is a pure
/// software plan, not a permission to mutate BlueZ or admit membership.
/// Both ends must have an observed neighbor edge; unknown NAP/PANU support,
/// adapter usability or NAP capacity is never interpreted as supported.
pub fn select_nap_hubs(inputs: &TopologyPolicyInputs) -> Result<NapSelection, CoreError> {
    use std::cmp::Reverse;
    use std::collections::BTreeSet;

    inputs.validate()?;
    let mut unserved: BTreeSet<NodeId> = inputs.candidates.keys().copied().collect();
    let mut selected_hubs = BTreeSet::new();
    let mut stars = Vec::new();

    loop {
        let mut best = None;
        for (id, candidate) in &inputs.candidates {
            if selected_hubs.contains(id)
                || candidate.capabilities.can_host_pan() != Some(true)
                || candidate
                    .capabilities
                    .adapter_usable
                    .as_ref()
                    .map(|v| v.value)
                    != Some(true)
            {
                continue;
            }
            let Some(capacity) = candidate
                .capabilities
                .connection_policy_ceiling
                .as_ref()
                .map(|ceiling| usize::from(ceiling.value))
            else {
                continue;
            };
            if capacity == 0 {
                continue;
            }
            let mut clients: Vec<(NodeId, CandidateLink)> = candidate
                .neighbors
                .iter()
                .filter_map(|(peer, link)| {
                    let remote = inputs.candidates.get(peer)?;
                    if !unserved.contains(peer)
                        || remote.capabilities.can_join_pan() != Some(true)
                        || remote.capabilities.adapter_usable.as_ref().map(|v| v.value)
                            != Some(true)
                    {
                        return None;
                    }
                    let reverse = remote.neighbors.get(id)?;
                    // A path is known stable only when both observed ends
                    // report stability. A one-sided optimistic observation
                    // must not displace a reciprocally stable client.
                    Some((
                        *peer,
                        CandidateLink {
                            stable: link.stable && reverse.stable,
                            ..*link
                        },
                    ))
                })
                .collect();
            clients.sort_by_key(|(peer, link)| {
                (
                    Reverse(link.stable),
                    Reverse(link.quality.map(|quality| quality.score).unwrap_or(0)),
                    link.path_cost,
                    *peer,
                )
            });
            clients.truncate(capacity);
            if clients.is_empty() {
                continue;
            }
            let stable = clients.iter().filter(|(_, link)| link.stable).count();
            let cost: u64 = clients
                .iter()
                .map(|(_, link)| u64::from(link.path_cost))
                .sum();
            let ac_power = candidate
                .capabilities
                .power_state
                .as_ref()
                .is_some_and(|power| matches!(power.value, PowerState::AcPowered));
            // For equal client coverage, retaining a currently working hub
            // avoids needless role churn. An unusable incumbent was filtered
            // above, so failover still selects another capable hub.
            let score = (
                clients.len(),
                candidate.currently_hosting,
                stable,
                ac_power,
                candidate.gateway_preferred,
                Reverse(cost),
                Reverse(*id),
            );
            if best
                .as_ref()
                .is_none_or(|(_, _, previous)| score > *previous)
            {
                best = Some((*id, clients, score));
            }
        }
        let Some((hub, clients, _)) = best else {
            break;
        };
        selected_hubs.insert(hub);
        unserved.remove(&hub);
        let clients: Vec<NodeId> = clients
            .into_iter()
            .map(|(node, _)| {
                unserved.remove(&node);
                node
            })
            .collect();
        stars.push(NapAssignment { hub, clients });
    }
    Ok(NapSelection { stars, unserved })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilitySource, Sourced};

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    #[test]
    fn heterogeneous_policy_inputs_validate_without_hardware_models() {
        let hub = TopologyCandidate {
            id: node(1),
            capabilities: NodeCapabilities {
                nap: Some(Sourced::new(true, CapabilitySource::Measured)),
                connection_policy_ceiling: Some(Sourced::new(3, CapabilitySource::Measured)),
                power_state: Some(Sourced::new(
                    PowerState::AcPowered,
                    CapabilitySource::Measured,
                )),
                ..NodeCapabilities::default()
            },
            neighbors: BTreeMap::from([(
                node(2),
                CandidateLink {
                    path_cost: 7,
                    quality: None,
                    stable: true,
                },
            )]),
            currently_hosting: true,
            gateway_preferred: false,
        };
        let client = TopologyCandidate {
            id: node(2),
            capabilities: NodeCapabilities {
                nap: Some(Sourced::new(false, CapabilitySource::Measured)),
                panu: Some(Sourced::new(true, CapabilitySource::Measured)),
                ..NodeCapabilities::default()
            },
            neighbors: BTreeMap::new(),
            currently_hosting: false,
            gateway_preferred: false,
        };
        let mut inputs = TopologyPolicyInputs {
            candidates: BTreeMap::from([(node(1), hub), (node(2), client)]),
        };
        assert!(inputs.validate().is_ok());
        inputs
            .candidates
            .get_mut(&node(1))
            .unwrap()
            .neighbors
            .get_mut(&node(2))
            .unwrap()
            .path_cost = 0;
        assert!(inputs.validate().is_err());
        inputs
            .candidates
            .get_mut(&node(1))
            .unwrap()
            .neighbors
            .get_mut(&node(2))
            .unwrap()
            .path_cost = 1;
        inputs
            .candidates
            .get_mut(&node(1))
            .unwrap()
            .neighbors
            .get_mut(&node(2))
            .unwrap()
            .quality = Some(LinkQuality::new(101));
        assert!(inputs.validate().is_err());
        inputs
            .candidates
            .get_mut(&node(1))
            .unwrap()
            .neighbors
            .clear();
        inputs
            .candidates
            .get_mut(&node(1))
            .unwrap()
            .capabilities
            .connection_policy_ceiling
            .as_mut()
            .unwrap()
            .value = 0;
        assert!(inputs.validate().is_err());
    }
    fn candidate(value: u8, nap: bool, panu: bool, capacity: Option<u16>) -> TopologyCandidate {
        TopologyCandidate {
            id: node(value),
            capabilities: NodeCapabilities {
                adapter_usable: Some(Sourced::new(true, CapabilitySource::Measured)),
                nap: Some(Sourced::new(nap, CapabilitySource::Measured)),
                panu: Some(Sourced::new(panu, CapabilitySource::Measured)),
                connection_policy_ceiling: capacity
                    .map(|n| Sourced::new(n, CapabilitySource::Measured)),
                ..NodeCapabilities::default()
            },
            neighbors: BTreeMap::new(),
            currently_hosting: false,
            gateway_preferred: false,
        }
    }

    fn connect(inputs: &mut TopologyPolicyInputs, a: u8, b: u8, cost: u32) {
        let link = CandidateLink {
            path_cost: cost,
            quality: None,
            stable: true,
        };
        inputs
            .candidates
            .get_mut(&node(a))
            .unwrap()
            .neighbors
            .insert(node(b), link);
        inputs
            .candidates
            .get_mut(&node(b))
            .unwrap()
            .neighbors
            .insert(node(a), link);
    }

    #[test]
    fn selection_prefers_one_capable_hub_and_never_assigns_client_only_as_nap() {
        let mut inputs = TopologyPolicyInputs {
            candidates: BTreeMap::from([
                (node(1), candidate(1, true, true, Some(3))),
                (node(2), candidate(2, false, true, None)),
                (node(3), candidate(3, false, true, None)),
                (node(4), candidate(4, false, true, None)),
            ]),
        };
        for peer in [2, 3, 4] {
            connect(&mut inputs, 1, peer, 1);
        }
        let selected = select_nap_hubs(&inputs).unwrap();
        assert_eq!(selected.stars.len(), 1);
        assert_eq!(selected.stars[0].hub, node(1));
        assert_eq!(selected.stars[0].clients, vec![node(2), node(3), node(4)]);
        assert!(selected.unserved.is_empty());
    }

    #[test]
    fn capacity_unknown_and_one_sided_links_fail_closed() {
        let mut inputs = TopologyPolicyInputs {
            candidates: BTreeMap::from([
                (node(1), candidate(1, true, false, None)),
                (node(2), candidate(2, false, true, None)),
                (node(3), candidate(3, false, true, None)),
            ]),
        };
        connect(&mut inputs, 1, 2, 1);
        connect(&mut inputs, 1, 3, 2);
        assert!(select_nap_hubs(&inputs).unwrap().stars.is_empty());
        inputs
            .candidates
            .get_mut(&node(1))
            .unwrap()
            .capabilities
            .connection_policy_ceiling = Some(Sourced::new(1, CapabilitySource::Measured));
        inputs
            .candidates
            .get_mut(&node(3))
            .unwrap()
            .neighbors
            .remove(&node(1));
        let selected = select_nap_hubs(&inputs).unwrap();
        assert_eq!(selected.stars.len(), 1);
        assert_eq!(selected.stars[0].clients, vec![node(2)]);
        assert!(selected.unserved.contains(&node(3)));
        assert!(!selected.unserved.contains(&node(1)));
    }

    #[test]
    fn known_good_hub_wins_equal_coverage_but_unusable_host_fails_over() {
        let mut inputs = TopologyPolicyInputs {
            candidates: BTreeMap::from([
                (node(1), candidate(1, true, false, Some(1))),
                (node(2), candidate(2, true, false, Some(1))),
                (node(3), candidate(3, false, true, None)),
            ]),
        };
        connect(&mut inputs, 1, 3, 10);
        connect(&mut inputs, 2, 3, 1);
        let incumbent = inputs.candidates.get_mut(&node(1)).unwrap();
        incumbent.currently_hosting = true;
        incumbent.neighbors.get_mut(&node(3)).unwrap().stable = false;

        let proposal = select_nap_hubs(&inputs).unwrap();
        assert_eq!(proposal.stars.len(), 1);
        assert_eq!(proposal.stars[0].hub, node(1));
        assert_eq!(proposal.stars[0].clients, vec![node(3)]);

        // Retention cannot override an affirmative capability/health denial.
        inputs
            .candidates
            .get_mut(&node(1))
            .unwrap()
            .capabilities
            .adapter_usable = Some(Sourced::new(false, CapabilitySource::Measured));
        let recovered = select_nap_hubs(&inputs).unwrap();
        assert_eq!(recovered.stars.len(), 1);
        assert_eq!(recovered.stars[0].hub, node(2));
        assert_eq!(recovered.stars[0].clients, vec![node(3)]);
    }
    #[test]
    fn stable_client_preference_requires_reciprocal_observation() {
        let mut inputs = TopologyPolicyInputs {
            candidates: BTreeMap::from([
                (node(1), candidate(1, true, false, Some(1))),
                (node(2), candidate(2, false, true, None)),
                (node(3), candidate(3, false, true, None)),
            ]),
        };
        connect(&mut inputs, 1, 2, 1);
        connect(&mut inputs, 1, 3, 10);

        // The hub sees both links as stable, but the first client does not.
        // Prefer the more expensive reciprocally stable path at capacity one.
        inputs
            .candidates
            .get_mut(&node(2))
            .unwrap()
            .neighbors
            .get_mut(&node(1))
            .unwrap()
            .stable = false;
        let selected = select_nap_hubs(&inputs).unwrap();
        assert_eq!(selected.stars.len(), 1);
        assert_eq!(selected.stars[0].clients, vec![node(3)]);
        assert!(selected.unserved.contains(&node(2)));

        // A confirmed reciprocal observation restores the lower-cost choice.
        inputs
            .candidates
            .get_mut(&node(2))
            .unwrap()
            .neighbors
            .get_mut(&node(1))
            .unwrap()
            .stable = true;
        let recovered = select_nap_hubs(&inputs).unwrap();
        assert_eq!(recovered.stars[0].clients, vec![node(2)]);
    }
}

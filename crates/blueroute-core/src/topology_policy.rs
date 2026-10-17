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
}

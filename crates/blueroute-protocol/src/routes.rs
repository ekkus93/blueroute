//! Route assignment message (P8-005). The network authority tells a member which routes to
//! install. Receivers must run `blueroute_core::validate_route_assignment` before applying.

use blueroute_core::{NetworkId, NodeId, Route};
use serde::{Deserialize, Serialize};

use crate::control::{ControlEnvelope, ControlVersion, MessageType};

pub const ROUTES_ASSIGN: &str = "routes.assign";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteAssignment {
    /// The complete desired set for the recipient; anything BlueRoute owns and not listed is
    /// stale and must be removed.
    pub routes: Vec<Route>,
}

impl RouteAssignment {
    pub fn to_envelope(
        &self,
        network: NetworkId,
        sender: NodeId,
        sequence: u64,
    ) -> ControlEnvelope {
        ControlEnvelope {
            version: ControlVersion::CURRENT,
            network,
            sender,
            sequence,
            message_type: MessageType::new(ROUTES_ASSIGN).expect("static type is valid"),
            critical: true,
            payload: serde_json::to_value(self).expect("assignments always serialize"),
        }
    }

    pub fn from_envelope(envelope: &ControlEnvelope) -> Option<Self> {
        if envelope.message_type.as_str() != ROUTES_ASSIGN {
            return None;
        }
        serde_json::from_value(envelope.payload.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use blueroute_core::{RouteDestination, RouteOwner, SegmentId};

    use super::*;

    #[test]
    fn assignment_round_trips_and_rejects_other_types_and_garbage() {
        let network = NetworkId::from_bytes([1; 16]);
        let sender = NodeId::from_bytes([2; 16]);
        let assignment = RouteAssignment {
            routes: vec![Route {
                destination: RouteDestination::Segment(SegmentId::from_bytes([3; 16])),
                next_hop: NodeId::from_bytes([4; 16]),
                cost: 2,
                owner: RouteOwner::BlueRouteNetwork(network),
            }],
        };
        let envelope = assignment.to_envelope(network, sender, 7);
        assert!(envelope.message_type.is_known());
        assert_eq!(RouteAssignment::from_envelope(&envelope), Some(assignment));

        let mut other = envelope.clone();
        other.message_type = MessageType::new("hello").unwrap();
        assert_eq!(RouteAssignment::from_envelope(&other), None);
        let mut bad = envelope;
        bad.payload = serde_json::json!({"routes": "nope"});
        assert_eq!(RouteAssignment::from_envelope(&bad), None);
    }
}

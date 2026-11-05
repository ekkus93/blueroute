//! Join messages exchanged inside an authenticated control session (ADR 0001).
//!
//! A joining node asks the network authority for a membership credential. The authority
//! answers with a signed [`Credential`] or a denial. The authority identity is pinned by the
//! joiner through the handshake's expected-peer check, so a rogue advertiser cannot answer.

use blueroute_core::NetworkId;
use serde::{Deserialize, Serialize};

use crate::control::{ControlEnvelope, ControlVersion, MessageType};
use crate::crypto::Credential;

pub const JOIN_REQUEST: &str = "join.request";
pub const JOIN_CREDENTIAL: &str = "join.credential";
pub const JOIN_DENIED: &str = "join.denied";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyReason {
    /// The node has not been approved by the authority's user.
    NotApproved,
    /// The request named a network this authority does not serve.
    UnknownNetwork,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JoinMessage {
    Request,
    Credential { credential: Credential },
    Denied { reason: DenyReason },
}

impl JoinMessage {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Request => JOIN_REQUEST,
            Self::Credential { .. } => JOIN_CREDENTIAL,
            Self::Denied { .. } => JOIN_DENIED,
        }
    }

    pub fn to_envelope(
        &self,
        network: NetworkId,
        sender: blueroute_core::NodeId,
        sequence: u64,
    ) -> ControlEnvelope {
        ControlEnvelope {
            version: ControlVersion::CURRENT,
            network,
            sender,
            sequence,
            message_type: MessageType::new(self.type_name()).expect("static join type is valid"),
            critical: true,
            payload: serde_json::to_value(self).expect("join messages always serialize"),
        }
    }

    /// Parse a validated envelope. The payload tag must agree with the envelope type.
    pub fn from_envelope(envelope: &ControlEnvelope) -> Option<Self> {
        let message: Self = serde_json::from_value(envelope.payload.clone()).ok()?;
        (message.type_name() == envelope.message_type.as_str()).then_some(message)
    }
}

#[cfg(test)]
mod tests {
    use blueroute_core::NodeId;

    use super::*;

    #[test]
    fn messages_round_trip_through_envelopes() {
        let network = NetworkId::from_bytes([1; 16]);
        let node = NodeId::from_bytes([2; 16]);
        for message in [
            JoinMessage::Request,
            JoinMessage::Denied {
                reason: DenyReason::NotApproved,
            },
        ] {
            let envelope = message.to_envelope(network, node, 3);
            assert!(envelope.message_type.is_known());
            assert_eq!(JoinMessage::from_envelope(&envelope), Some(message));
        }
    }

    #[test]
    fn mismatched_tag_and_type_is_rejected() {
        let network = NetworkId::from_bytes([1; 16]);
        let node = NodeId::from_bytes([2; 16]);
        let mut envelope = JoinMessage::Request.to_envelope(network, node, 1);
        envelope.message_type = MessageType::new(JOIN_DENIED).unwrap();
        assert_eq!(JoinMessage::from_envelope(&envelope), None);
        envelope.payload = serde_json::json!({"kind": "bogus"});
        assert_eq!(JoinMessage::from_envelope(&envelope), None);
    }
}

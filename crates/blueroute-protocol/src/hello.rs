//! Peer hello and capability exchange (P7-003).
//!
//! The first message of an authenticated control session. Each side states its software and
//! protocol version, the identity it claims, optional feature flags, the capabilities the
//! node has measured or configured, and the PAN segment facts it currently knows. Peers
//! negotiate on capabilities and features only; nothing here identifies a computer model.

use std::collections::BTreeSet;
use std::fmt;

use blueroute_core::{IpPrefix, NetworkId, NodeCapabilities, NodeId, SegmentId};
use serde::{Deserialize, Serialize};

use crate::control::{ControlEnvelope, ControlVersion, MessageType};

pub const MAX_SOFTWARE_VERSION_BYTES: usize = 64;
pub const MAX_FEATURES: usize = 32;
pub const MAX_FEATURE_BYTES: usize = 32;
pub const MAX_SEGMENT_FACTS: usize = 16;

/// What this node does on a PAN segment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentRole {
    Host,
    Client,
}

/// One segment the sender participates in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SegmentFact {
    pub segment: SegmentId,
    pub role: SegmentRole,
    pub prefix: Option<IpPrefix>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Hello {
    /// Informational only (diagnostics); never used for decisions.
    pub software: String,
    pub protocol: ControlVersion,
    /// Claimed identity. Must equal the session's authenticated peer.
    pub node: NodeId,
    /// Optional protocol features; unknown ones are ignored by the peer.
    #[serde(default)]
    pub features: BTreeSet<String>,
    #[serde(default)]
    pub capabilities: NodeCapabilities,
    #[serde(default)]
    pub segments: Vec<SegmentFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HelloError {
    NotHello,
    Malformed(String),
    IdentityMismatch,
    IncompatibleMajor { local: u16, remote: u16 },
    InvalidField(&'static str),
}

impl fmt::Display for HelloError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHello => f.write_str("first control message is not a hello"),
            Self::Malformed(reason) => write!(f, "malformed hello: {reason}"),
            Self::IdentityMismatch => {
                f.write_str("hello identity differs from the authenticated session peer")
            }
            Self::IncompatibleMajor { local, remote } => {
                write!(
                    f,
                    "incompatible control protocol major (local {local}, peer {remote})"
                )
            }
            Self::InvalidField(field) => write!(f, "hello field `{field}` is invalid or too large"),
        }
    }
}

impl std::error::Error for HelloError {}

fn valid_feature(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FEATURE_BYTES
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
        })
}

impl Hello {
    /// Check bounds on every peer-controlled field.
    pub fn validate(&self) -> Result<(), HelloError> {
        if self.software.is_empty()
            || self.software.len() > MAX_SOFTWARE_VERSION_BYTES
            || self.software.chars().any(char::is_control)
        {
            return Err(HelloError::InvalidField("software"));
        }
        if self.features.len() > MAX_FEATURES || !self.features.iter().all(|f| valid_feature(f)) {
            return Err(HelloError::InvalidField("features"));
        }
        if self.segments.len() > MAX_SEGMENT_FACTS {
            return Err(HelloError::InvalidField("segments"));
        }
        Ok(())
    }

    pub fn to_envelope(
        &self,
        network: NetworkId,
        sequence: u64,
    ) -> Result<ControlEnvelope, HelloError> {
        self.validate()?;
        let payload =
            serde_json::to_value(self).map_err(|e| HelloError::Malformed(e.to_string()))?;
        Ok(ControlEnvelope {
            version: ControlVersion::CURRENT,
            network,
            sender: self.node,
            sequence,
            message_type: MessageType::new(MessageType::HELLO)
                .map_err(|e| HelloError::Malformed(e.to_string()))?,
            critical: true,
            payload,
        })
    }

    /// Parse the payload of an already validated envelope.
    pub fn from_envelope(envelope: &ControlEnvelope) -> Result<Self, HelloError> {
        if envelope.message_type.as_str() != MessageType::HELLO {
            return Err(HelloError::NotHello);
        }
        let hello: Self = serde_json::from_value(envelope.payload.clone())
            .map_err(|e| HelloError::Malformed(e.to_string()))?;
        hello.validate()?;
        if hello.node != envelope.sender {
            return Err(HelloError::IdentityMismatch);
        }
        Ok(hello)
    }
}

/// Outcome of comparing two hellos.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Negotiated {
    pub peer: NodeId,
    /// Same major, lower of the two minors.
    pub protocol: ControlVersion,
    pub shared_features: BTreeSet<String>,
    pub peer_capabilities: NodeCapabilities,
    pub peer_segments: Vec<SegmentFact>,
    pub peer_software: String,
}

/// Negotiate using only versions and declared features. The peer's capabilities and segment
/// facts are carried through for later topology decisions, never interpreted by model.
pub fn negotiate(local: &Hello, remote: &Hello) -> Result<Negotiated, HelloError> {
    if local.protocol.major != remote.protocol.major {
        return Err(HelloError::IncompatibleMajor {
            local: local.protocol.major,
            remote: remote.protocol.major,
        });
    }
    Ok(Negotiated {
        peer: remote.node,
        protocol: ControlVersion {
            major: local.protocol.major,
            minor: local.protocol.minor.min(remote.protocol.minor),
        },
        shared_features: local
            .features
            .intersection(&remote.features)
            .cloned()
            .collect(),
        peer_capabilities: remote.capabilities.clone(),
        peer_segments: remote.segments.clone(),
        peer_software: remote.software.clone(),
    })
}

#[cfg(test)]
mod tests {
    use blueroute_core::{CapabilitySource, Sourced};

    use super::*;
    use crate::control::{Inbound, InboundValidator};

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([9; 16])
    }

    fn hello(b: u8, minor: u16, features: &[&str]) -> Hello {
        Hello {
            software: "blueroute 0.1.0".into(),
            protocol: ControlVersion { major: 1, minor },
            node: node(b),
            features: features.iter().map(|f| (*f).to_owned()).collect(),
            capabilities: NodeCapabilities {
                nap: Some(Sourced::new(b == 1, CapabilitySource::Measured)),
                ..NodeCapabilities::default()
            },
            segments: vec![SegmentFact {
                segment: SegmentId::from_bytes([5; 16]),
                role: SegmentRole::Host,
                prefix: None,
            }],
        }
    }

    #[test]
    fn hello_round_trips_through_a_validated_envelope() {
        let sent = hello(1, 0, &["routes"]);
        let envelope = sent.to_envelope(network(), 1).unwrap();
        let mut validator = InboundValidator::new(network(), node(1));
        let Inbound::Handle(accepted) = validator.accept(envelope).unwrap() else {
            panic!("hello must be handled");
        };
        assert_eq!(Hello::from_envelope(&accepted).unwrap(), sent);
    }

    #[test]
    fn peers_negotiate_without_any_model_knowledge() {
        let a = hello(1, 2, &["routes", "topology", "future-x"]);
        let b = hello(2, 1, &["routes", "topology"]);
        let n = negotiate(&a, &b).unwrap();
        assert_eq!(n.protocol, ControlVersion { major: 1, minor: 1 });
        assert_eq!(n.shared_features.len(), 2);
        assert_eq!(n.peer_capabilities.can_host_pan(), Some(false));
        assert_eq!(
            negotiate(&b, &a).unwrap().peer_capabilities.can_host_pan(),
            Some(true)
        );
        assert_eq!(n.peer_segments.len(), 1);
    }

    #[test]
    fn major_mismatch_is_rejected() {
        let a = hello(1, 0, &[]);
        let mut b = hello(2, 0, &[]);
        b.protocol.major = 2;
        assert_eq!(
            negotiate(&a, &b),
            Err(HelloError::IncompatibleMajor {
                local: 1,
                remote: 2
            })
        );
    }

    #[test]
    fn claimed_identity_must_match_authenticated_sender() {
        let mut envelope = hello(1, 0, &[]).to_envelope(network(), 1).unwrap();
        envelope.sender = node(2);
        assert_eq!(
            Hello::from_envelope(&envelope),
            Err(HelloError::IdentityMismatch)
        );
    }

    #[test]
    fn non_hello_and_malformed_payloads_are_rejected() {
        let mut envelope = hello(1, 0, &[]).to_envelope(network(), 1).unwrap();
        envelope.payload = serde_json::json!({ "node": 7 });
        assert!(matches!(
            Hello::from_envelope(&envelope),
            Err(HelloError::Malformed(_))
        ));
        envelope.message_type = MessageType::new("other").unwrap();
        assert_eq!(Hello::from_envelope(&envelope), Err(HelloError::NotHello));
    }

    #[test]
    fn peer_controlled_fields_are_bounded() {
        let mut h = hello(1, 0, &[]);
        h.software = "x".repeat(MAX_SOFTWARE_VERSION_BYTES + 1);
        assert_eq!(h.validate(), Err(HelloError::InvalidField("software")));
        let mut h = hello(1, 0, &["Bad Feature"]);
        assert_eq!(h.validate(), Err(HelloError::InvalidField("features")));
        h.features.clear();
        h.segments = (0..=MAX_SEGMENT_FACTS)
            .map(|_| SegmentFact {
                segment: SegmentId::from_bytes([1; 16]),
                role: SegmentRole::Client,
                prefix: None,
            })
            .collect();
        assert_eq!(h.validate(), Err(HelloError::InvalidField("segments")));
    }
}

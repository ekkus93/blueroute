//! Inter-node control-plane envelope (P7-002, ADR 0001).
//!
//! Frames travel inside an authenticated Noise session. This module is transport- and
//! crypto-free: it defines the wire envelope, the bounded framing, and the inbound validator
//! that enforces version, identity, criticality, and freshness rules. Everything here treats
//! peer bytes as hostile and returns typed errors instead of panicking.

use std::fmt;

use blueroute_core::{NetworkId, NodeId};
use serde::{Deserialize, Serialize};

/// Largest accepted frame body in bytes. Larger frames are rejected before any parsing.
pub const MAX_CONTROL_FRAME_BYTES: usize = 16 * 1024;
/// Number of bytes in the big-endian length prefix.
pub const CONTROL_FRAME_HEADER_BYTES: usize = 4;
/// Longest accepted message-type name.
pub const MAX_MESSAGE_TYPE_BYTES: usize = 32;

/// Control protocol version. Majors are incompatible; minors are additive.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlVersion {
    pub major: u16,
    pub minor: u16,
}

impl ControlVersion {
    pub const CURRENT: Self = Self { major: 1, minor: 0 };
}

/// Validated lowercase ASCII message-type name (`[a-z0-9_.-]`, 1..=32 bytes).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct MessageType(String);

impl MessageType {
    pub const HELLO: &'static str = "hello";

    pub fn new(name: impl Into<String>) -> Result<Self, ControlError> {
        let name = name.into();
        let valid = !name.is_empty()
            && name.len() <= MAX_MESSAGE_TYPE_BYTES
            && name.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
            });
        if valid {
            Ok(Self(name))
        } else {
            Err(ControlError::InvalidMessageType)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this daemon version understands the message type.
    pub fn is_known(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::HELLO | "join.request" | "join.credential" | "join.denied"
        )
    }
}

impl TryFrom<String> for MessageType {
    type Error = ControlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MessageType> for String {
    fn from(value: MessageType) -> Self {
        value.0
    }
}

/// One control message. The payload is opaque JSON bounded by the frame limit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlEnvelope {
    pub version: ControlVersion,
    pub network: NetworkId,
    pub sender: NodeId,
    /// Strictly increasing per sender and session.
    pub sequence: u64,
    pub message_type: MessageType,
    /// A receiver that does not understand a critical message must reject the session.
    #[serde(default)]
    pub critical: bool,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlError {
    Oversized { declared: usize },
    Truncated,
    TrailingBytes,
    Malformed(String),
    InvalidMessageType,
    UnsupportedMajorVersion { received: u16 },
    UnknownCritical(String),
    WrongNetwork,
    WrongSender,
    NonIncreasingSequence { last: u64, received: u64 },
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversized { declared } => write!(
                f,
                "control frame of {declared} bytes exceeds the {MAX_CONTROL_FRAME_BYTES}-byte limit"
            ),
            Self::Truncated => f.write_str("control frame is truncated"),
            Self::TrailingBytes => f.write_str("control frame has trailing bytes"),
            Self::Malformed(reason) => write!(f, "malformed control frame: {reason}"),
            Self::InvalidMessageType => f.write_str("invalid control message type"),
            Self::UnsupportedMajorVersion { received } => {
                write!(f, "unsupported control protocol major version {received}")
            }
            Self::UnknownCritical(kind) => {
                write!(f, "unknown critical control message type `{kind}`")
            }
            Self::WrongNetwork => f.write_str("control message is for a different network"),
            Self::WrongSender => f.write_str("control message sender is not the session peer"),
            Self::NonIncreasingSequence { last, received } => write!(
                f,
                "control sequence {received} does not exceed previous {last} (replay or reorder)"
            ),
        }
    }
}

impl std::error::Error for ControlError {}

/// Encode one envelope as `u32_be(length) || json`.
pub fn encode_frame(envelope: &ControlEnvelope) -> Result<Vec<u8>, ControlError> {
    let body = serde_json::to_vec(envelope).map_err(|e| ControlError::Malformed(e.to_string()))?;
    if body.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(ControlError::Oversized {
            declared: body.len(),
        });
    }
    let mut frame = Vec::with_capacity(CONTROL_FRAME_HEADER_BYTES + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Read a frame header and return the declared body length, rejecting oversized frames.
///
/// Callers should call this on the first four bytes before buffering the body so a hostile
/// peer cannot make the daemon allocate beyond [`MAX_CONTROL_FRAME_BYTES`].
pub fn frame_body_len(header: &[u8]) -> Result<usize, ControlError> {
    let header: [u8; CONTROL_FRAME_HEADER_BYTES] = header
        .get(..CONTROL_FRAME_HEADER_BYTES)
        .and_then(|h| h.try_into().ok())
        .ok_or(ControlError::Truncated)?;
    let declared = u32::from_be_bytes(header) as usize;
    if declared > MAX_CONTROL_FRAME_BYTES {
        Err(ControlError::Oversized { declared })
    } else {
        Ok(declared)
    }
}

/// Decode exactly one complete frame.
pub fn decode_frame(frame: &[u8]) -> Result<ControlEnvelope, ControlError> {
    let body_len = frame_body_len(frame)?;
    let body = frame
        .get(CONTROL_FRAME_HEADER_BYTES..)
        .ok_or(ControlError::Truncated)?;
    if body.len() < body_len {
        return Err(ControlError::Truncated);
    }
    if body.len() > body_len {
        return Err(ControlError::TrailingBytes);
    }
    serde_json::from_slice(body).map_err(|e| ControlError::Malformed(e.to_string()))
}

/// What the receiver should do with a validated envelope.
#[derive(Debug, Eq, PartialEq)]
pub enum Inbound {
    /// A message type this daemon understands.
    Handle(ControlEnvelope),
    /// An unknown but non-critical message that is safely ignored.
    Ignored { message_type: MessageType },
}

/// Per-session inbound validator. Create one per authenticated session, bound to the
/// network and the authenticated peer identity established by the handshake.
#[derive(Debug)]
pub struct InboundValidator {
    network: NetworkId,
    peer: NodeId,
    last_sequence: Option<u64>,
}

impl InboundValidator {
    pub const fn new(network: NetworkId, peer: NodeId) -> Self {
        Self {
            network,
            peer,
            last_sequence: None,
        }
    }

    /// Validate version, binding, criticality, and freshness. Sequence state advances only
    /// when the envelope is accepted, so a rejected message cannot desynchronize the session.
    pub fn accept(&mut self, envelope: ControlEnvelope) -> Result<Inbound, ControlError> {
        if envelope.version.major != ControlVersion::CURRENT.major {
            return Err(ControlError::UnsupportedMajorVersion {
                received: envelope.version.major,
            });
        }
        if envelope.network != self.network {
            return Err(ControlError::WrongNetwork);
        }
        if envelope.sender != self.peer {
            return Err(ControlError::WrongSender);
        }
        if let Some(last) = self.last_sequence
            && envelope.sequence <= last
        {
            return Err(ControlError::NonIncreasingSequence {
                last,
                received: envelope.sequence,
            });
        }
        if !envelope.message_type.is_known() && envelope.critical {
            return Err(ControlError::UnknownCritical(
                envelope.message_type.as_str().to_owned(),
            ));
        }

        self.last_sequence = Some(envelope.sequence);
        if envelope.message_type.is_known() {
            Ok(Inbound::Handle(envelope))
        } else {
            Ok(Inbound::Ignored {
                message_type: envelope.message_type,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> NetworkId {
        NetworkId::from_bytes([1; 16])
    }

    fn peer() -> NodeId {
        NodeId::from_bytes([2; 16])
    }

    fn envelope(sequence: u64, kind: &str, critical: bool) -> ControlEnvelope {
        ControlEnvelope {
            version: ControlVersion::CURRENT,
            network: network(),
            sender: peer(),
            sequence,
            message_type: MessageType::new(kind).unwrap(),
            critical,
            payload: serde_json::json!({"k": "v"}),
        }
    }

    #[test]
    fn frames_round_trip() {
        let original = envelope(1, "hello", false);
        let frame = encode_frame(&original).unwrap();
        assert_eq!(decode_frame(&frame).unwrap(), original);
    }

    #[test]
    fn oversized_payload_is_rejected_on_encode_and_declared_length_on_decode() {
        let mut big = envelope(1, "hello", false);
        big.payload = serde_json::Value::String("x".repeat(MAX_CONTROL_FRAME_BYTES));
        assert!(matches!(
            encode_frame(&big),
            Err(ControlError::Oversized { .. })
        ));

        let hostile = (MAX_CONTROL_FRAME_BYTES as u32 + 1).to_be_bytes();
        assert!(matches!(
            frame_body_len(&hostile),
            Err(ControlError::Oversized { .. })
        ));
        assert!(matches!(
            decode_frame(&u32::MAX.to_be_bytes()),
            Err(ControlError::Oversized { .. })
        ));
    }

    #[test]
    fn truncated_trailing_and_malformed_frames_are_rejected() {
        let frame = encode_frame(&envelope(1, "hello", false)).unwrap();
        assert_eq!(decode_frame(&frame[..2]), Err(ControlError::Truncated));
        assert_eq!(
            decode_frame(&frame[..frame.len() - 1]),
            Err(ControlError::Truncated)
        );
        let mut trailing = frame.clone();
        trailing.push(0);
        assert_eq!(decode_frame(&trailing), Err(ControlError::TrailingBytes));

        let mut malformed = 3_u32.to_be_bytes().to_vec();
        malformed.extend_from_slice(b"{no");
        assert!(matches!(
            decode_frame(&malformed),
            Err(ControlError::Malformed(_))
        ));
        assert_eq!(decode_frame(&[]), Err(ControlError::Truncated));
    }

    #[test]
    fn message_type_validation_rejects_hostile_names() {
        for bad in [
            "",
            "Hello",
            "has space",
            "semi;colon",
            "ünï",
            &"a".repeat(33),
        ] {
            assert_eq!(
                MessageType::new(bad),
                Err(ControlError::InvalidMessageType),
                "{bad:?}"
            );
        }
        assert!(MessageType::new("route.update-v2_x").is_ok());

        let json = br#"{"version":{"major":1,"minor":0},"network":"01010101010101010101010101010101","sender":"02020202020202020202020202020202","sequence":1,"message_type":"BAD NAME","payload":null}"#;
        let mut frame = (json.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(json);
        assert!(decode_frame(&frame).is_err());
    }

    #[test]
    fn validator_enforces_version_network_sender_and_freshness() {
        let mut validator = InboundValidator::new(network(), peer());

        let mut wrong_major = envelope(1, "hello", false);
        wrong_major.version.major = 2;
        assert_eq!(
            validator.accept(wrong_major),
            Err(ControlError::UnsupportedMajorVersion { received: 2 })
        );

        let mut other_network = envelope(1, "hello", false);
        other_network.network = NetworkId::from_bytes([9; 16]);
        assert_eq!(
            validator.accept(other_network),
            Err(ControlError::WrongNetwork)
        );

        let mut other_sender = envelope(1, "hello", false);
        other_sender.sender = NodeId::from_bytes([9; 16]);
        assert_eq!(
            validator.accept(other_sender),
            Err(ControlError::WrongSender)
        );

        // Rejections above must not have consumed sequence numbers.
        assert!(matches!(
            validator.accept(envelope(1, "hello", false)),
            Ok(Inbound::Handle(_))
        ));
        assert_eq!(
            validator.accept(envelope(1, "hello", false)),
            Err(ControlError::NonIncreasingSequence {
                last: 1,
                received: 1
            })
        );
        assert!(matches!(
            validator.accept(envelope(5, "hello", false)),
            Ok(Inbound::Handle(_))
        ));
        assert_eq!(
            validator.accept(envelope(3, "hello", false)),
            Err(ControlError::NonIncreasingSequence {
                last: 5,
                received: 3
            })
        );
    }

    #[test]
    fn unknown_critical_is_rejected_and_unknown_non_critical_is_ignored() {
        let mut validator = InboundValidator::new(network(), peer());
        assert_eq!(
            validator.accept(envelope(1, "future.thing", true)),
            Err(ControlError::UnknownCritical("future.thing".into()))
        );
        // The rejected critical message did not consume sequence 1.
        assert!(matches!(
            validator.accept(envelope(1, "future.thing", false)),
            Ok(Inbound::Ignored { .. })
        ));
        assert!(matches!(
            validator.accept(envelope(2, "hello", true)),
            Ok(Inbound::Handle(_))
        ));
    }

    #[test]
    fn newer_minor_version_from_same_major_is_accepted() {
        let mut validator = InboundValidator::new(network(), peer());
        let mut newer = envelope(1, "hello", false);
        newer.version.minor = 7;
        assert!(matches!(validator.accept(newer), Ok(Inbound::Handle(_))));
    }

    #[test]
    fn maximum_sequence_cannot_be_exceeded() {
        let mut validator = InboundValidator::new(network(), peer());
        assert!(validator.accept(envelope(u64::MAX, "hello", false)).is_ok());
        assert!(
            validator
                .accept(envelope(u64::MAX, "hello", false))
                .is_err()
        );
    }
}

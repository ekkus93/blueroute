//! Untrusted, bounded inter-node control-plane framing (not the local D-Bus API).
//!
//! A decoded message is NOT an authenticated peer. A TLS session must verify
//! the remote cryptographic identity and bind the expected NodeId, NetworkId
//! and session nonce to the channel before this data may authorize anything.

use std::fmt;

use blueroute_core::{IpPrefix, NetworkId, NodeCapabilities, NodeId, normalized_ipv4_prefix};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

/// Versioned independently from the local daemon API.
pub const CONTROL_VERSION: u16 = 1;
/// Maximum number of bytes in a serialized control payload.
pub const MAX_CONTROL_FRAME_BYTES: usize = 16 * 1024;
const HEADER_LEN: usize = 4;

/// Version, capabilities and topology facts advertised by an untrusted peer.
///
/// These are non-authoritative until TLS proves the peer key, its approved
/// logical identity, the expected network, and the entire session context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelloFacts {
    pub software_version: String,
    #[serde(deserialize_with = "deserialize_control_capabilities")]
    pub capabilities: NodeCapabilities,
    pub segment: Option<IpPrefix>,
    pub direct_neighbors: Vec<NodeId>,
}

// The local D-Bus API intentionally allows additive capability fields across
// compatible minor versions. The *inter-node security protocol* does not:
// unknown nested claims must be rejected, not silently interpreted as safe.
// Keep this strict wire boundary separate from the shared domain model.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictSourced<T> {
    value: T,
    source: blueroute_core::CapabilitySource,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictLinkQuality {
    score: u8,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum StrictPowerState {
    AcPowered,
    Battery { percent: u8 },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictControlCapabilities {
    adapter_usable: Option<StrictSourced<bool>>,
    panu: Option<StrictSourced<bool>>,
    nap: Option<StrictSourced<bool>>,
    routing: Option<StrictSourced<bool>>,
    network_backend: Option<StrictSourced<blueroute_core::NetworkBackend>>,
    connection_policy_ceiling: Option<StrictSourced<u16>>,
    link_quality: Option<StrictSourced<StrictLinkQuality>>,
    power_state: Option<StrictSourced<StrictPowerState>>,
    has_internet: Option<StrictSourced<bool>>,
    willing_to_share_internet: Option<StrictSourced<bool>>,
}

fn deserialize_control_capabilities<'de, D>(deserializer: D) -> Result<NodeCapabilities, D::Error>
where
    D: Deserializer<'de>,
{
    // Deserialize strictly *before* converting to the reusable domain type.
    // This rejects unknown and duplicate fields even several levels deep.
    let strict = StrictControlCapabilities::deserialize(deserializer)?;
    let value = serde_json::to_value(strict).map_err(serde::de::Error::custom)?;
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}

fn validate_facts(facts: &HelloFacts, sender: NodeId) -> Result<(), ControlFrameError> {
    if facts.software_version.is_empty()
        || facts.software_version.len() > 64
        || !facts
            .software_version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_'))
    {
        return Err(invalid("invalid software version"));
    }
    if facts.direct_neighbors.len() > 32 {
        return Err(invalid("too many direct neighbor claims"));
    }
    let mut ids = BTreeSet::new();
    for node in &facts.direct_neighbors {
        if *node == sender || !ids.insert(*node) {
            return Err(invalid("duplicated or self-referential neighbor"));
        }
    }
    if let Some(segment) = facts.segment
        && normalized_ipv4_prefix(segment).ok() != Some(segment)
    {
        return Err(invalid("hello requires canonical IPv4 segment"));
    }
    if facts
        .capabilities
        .link_quality
        .as_ref()
        .is_some_and(|quality| quality.value.score > 100)
    {
        return Err(invalid("link quality exceeds valid range"));
    }
    if facts
        .capabilities
        .connection_policy_ceiling
        .as_ref()
        .is_some_and(|ceiling| ceiling.value == 0)
    {
        return Err(invalid("zero peer connection policy ceiling"));
    }
    if facts
        .capabilities
        .power_state
        .as_ref()
        .is_some_and(|power| matches!(power.value, blueroute_core::PowerState::Battery { percent } if percent > 100))
    {
        return Err(invalid("battery percentage exceeds valid range"));
    }
    Ok(())
}

/// Initial bootstrap and liveness types only; no unauthenticated route changes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ControlMessage {
    Hello {
        challenge: [u8; 16],
        facts: HelloFacts,
    },
    HelloAck {
        challenge: [u8; 16],
        facts: HelloFacts,
    },
    Heartbeat,
    Goodbye,
}

/// Sender and network fields are claims until verified by the session layer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlEnvelope {
    pub version: u16,
    pub network: NetworkId,
    pub sender: NodeId,
    /// Per-session random nonce; the session layer must bind this to TLS.
    pub session_id: [u8; 16],
    /// Sequence numbers must increase within an authenticated session.
    pub sequence: u64,
    pub message: ControlMessage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlFrameError {
    message: String,
}

impl ControlFrameError {
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ControlFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ControlFrameError {}

fn invalid(message: impl Into<String>) -> ControlFrameError {
    ControlFrameError {
        message: message.into(),
    }
}

fn validate(envelope: &ControlEnvelope) -> Result<(), ControlFrameError> {
    if envelope.version != CONTROL_VERSION {
        return Err(invalid("unsupported control protocol version"));
    }
    if envelope.session_id.iter().all(|byte| *byte == 0) {
        return Err(invalid("zero session identifier"));
    }
    if envelope.sequence == 0 {
        return Err(invalid("zero control sequence"));
    }
    match &envelope.message {
        ControlMessage::Hello { challenge, facts }
        | ControlMessage::HelloAck { challenge, facts } => {
            if challenge.iter().all(|byte| *byte == 0) {
                return Err(invalid("zero hello challenge"));
            }
            validate_facts(facts, envelope.sender)?;
        }
        _ => {}
    }
    Ok(())
}

/// Emit a four-byte big-endian length followed by one strict JSON envelope.
/// This DOES NOT authenticate the supplied sender or network identity.
pub fn encode_control_frame(envelope: &ControlEnvelope) -> Result<Vec<u8>, ControlFrameError> {
    validate(envelope)?;
    let payload = serde_json::to_vec(envelope).map_err(|error| invalid(error.to_string()))?;
    if payload.is_empty() || payload.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(invalid("control frame exceeds maximum length"));
    }
    let len = u32::try_from(payload.len()).map_err(|_| invalid("length overflow"))?;
    let mut result = Vec::with_capacity(HEADER_LEN + payload.len());
    result.extend_from_slice(&len.to_be_bytes());
    result.extend_from_slice(&payload);
    Ok(result)
}

/// Inspect the untrusted size header before allocating the frame body.
pub fn read_control_length(bytes: &[u8]) -> Result<usize, ControlFrameError> {
    let header: [u8; HEADER_LEN] = bytes
        .get(..HEADER_LEN)
        .ok_or_else(|| invalid("truncated control length header"))?
        .try_into()
        .map_err(|_| invalid("invalid control length header"))?;
    let declared = u32::from_be_bytes(header) as usize;
    if declared == 0 || declared > MAX_CONTROL_FRAME_BYTES {
        return Err(invalid("invalid or oversized control frame length"));
    }
    Ok(declared)
}

/// Reject truncation, extra data, unknown message types/fields and bad version.
/// This function decodes exactly one complete frame and never trusts its peer.
pub fn decode_control_frame(framed: &[u8]) -> Result<ControlEnvelope, ControlFrameError> {
    let expected_len = read_control_length(framed)?;
    if framed.len() != HEADER_LEN + expected_len {
        return Err(invalid("truncated control frame or trailing bytes"));
    }
    let message: ControlEnvelope = serde_json::from_slice(&framed[HEADER_LEN..])
        .map_err(|error| invalid(format!("invalid control payload: {error}")))?;
    validate(&message)?;
    Ok(message)
}

/// Simple per-session replay protection; not a substitute for TLS authentication.
///
/// Caller must create this tracker only after cryptographically authenticating
/// the peer, verifying network approval and binding the session ID to the
/// established channel. No production JoinNetwork code uses this yet.
#[derive(Clone, Debug)]
pub struct VerifiedSessionSequence {
    network: NetworkId,
    sender: NodeId,
    session_id: [u8; 16],
    last_sequence: u64,
}

impl VerifiedSessionSequence {
    pub fn new(
        network: NetworkId,
        sender: NodeId,
        session_id: [u8; 16],
    ) -> Result<Self, ControlFrameError> {
        if session_id.iter().all(|byte| *byte == 0) {
            return Err(invalid("zero session identifier"));
        }
        Ok(Self {
            network,
            sender,
            session_id,
            last_sequence: 0,
        })
    }

    /// Reject duplicate, stale and wrong-network/peer/session messages.
    pub fn accept(&mut self, envelope: &ControlEnvelope) -> Result<(), ControlFrameError> {
        validate(envelope)?;
        if envelope.network != self.network
            || envelope.sender != self.sender
            || envelope.session_id != self.session_id
        {
            return Err(invalid("message differs from verified session context"));
        }
        if envelope.sequence <= self.last_sequence {
            return Err(invalid("replayed or stale control sequence"));
        }
        self.last_sequence = envelope.sequence;
        Ok(())
    }

    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_facts() -> HelloFacts {
        HelloFacts {
            software_version: "0.1.0".into(),
            capabilities: NodeCapabilities::default(),
            segment: None,
            direct_neighbors: vec![NodeId::from_bytes([7; 16])],
        }
    }

    fn sample() -> ControlEnvelope {
        ControlEnvelope {
            version: CONTROL_VERSION,
            network: NetworkId::from_bytes([2; 16]),
            sender: NodeId::from_bytes([3; 16]),
            session_id: [4; 16],
            sequence: 1,
            message: ControlMessage::Hello {
                challenge: [5; 16],
                facts: example_facts(),
            },
        }
    }

    #[test]
    fn deterministic_encoding_round_trips() {
        let envelope = sample();
        let encoded = encode_control_frame(&envelope).unwrap();
        assert_eq!(read_control_length(&encoded).unwrap(), encoded.len() - 4);
        assert_eq!(encode_control_frame(&envelope).unwrap(), encoded);
        assert_eq!(decode_control_frame(&encoded).unwrap(), envelope);
    }

    #[test]
    fn rejects_truncated_trailing_and_oversize_frames() {
        let mut frame = encode_control_frame(&sample()).unwrap();
        assert!(decode_control_frame(&frame[..3]).is_err());
        assert!(decode_control_frame(&frame[..frame.len() - 1]).is_err());
        frame.push(0);
        assert!(decode_control_frame(&frame).is_err());
        assert!(read_control_length(&[0, 0, 0, 0]).is_err());
        assert!(
            read_control_length(&((MAX_CONTROL_FRAME_BYTES as u32) + 1).to_be_bytes()).is_err()
        );
    }

    #[test]
    fn rejects_version_zero_session_sequence_and_challenge() {
        let mut envelope = sample();
        envelope.version += 1;
        assert!(encode_control_frame(&envelope).is_err());
        envelope.version = CONTROL_VERSION;
        envelope.sequence = 0;
        assert!(encode_control_frame(&envelope).is_err());
        envelope.sequence = 1;
        envelope.session_id = [0; 16];
        assert!(encode_control_frame(&envelope).is_err());
        envelope.session_id = [4; 16];
        envelope.message = ControlMessage::Hello {
            challenge: [0; 16],
            facts: example_facts(),
        };
        assert!(encode_control_frame(&envelope).is_err());
    }

    #[test]
    fn unknown_types_and_critical_fields_fail_closed() {
        let encoded = encode_control_frame(&sample()).unwrap();
        let json = String::from_utf8(encoded[4..].to_vec()).unwrap();
        let bad_type = json.replace("\"hello\"", "\"unauthorized_route\"");
        let bad_field = json.replace("\"sequence\":1,", "\"sequence\":1,\"superuser\":true,");
        let bad_hello_field = json.replace(
            "\"challenge\":",
            "\"critical_session_override\":true,\"challenge\":",
        );
        assert_ne!(bad_hello_field, json);
        for payload in [bad_type, bad_field, bad_hello_field] {
            let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
            framed.extend_from_slice(payload.as_bytes());
            assert!(decode_control_frame(&framed).is_err());
        }
    }

    #[test]
    fn unknown_nested_capability_fields_fail_closed_only_on_control_wire() {
        let encoded = encode_control_frame(&sample()).unwrap();
        let base: serde_json::Value = serde_json::from_slice(&encoded[4..]).unwrap();
        for path in 0..4 {
            let mut value = base.clone();
            let capabilities = &mut value["message"]["data"]["facts"]["capabilities"];
            match path {
                0 => capabilities["unknown_critical"] = serde_json::json!(true),
                1 => {
                    capabilities["nap"] = serde_json::json!({
                        "value": true, "source": "discovered", "unexpected": 1
                    });
                }
                2 => {
                    capabilities["link_quality"] = serde_json::json!({
                        "value": { "score": 50, "override": true },
                        "source": "measured"
                    });
                }
                _ => {
                    capabilities["power_state"] = serde_json::json!({
                        "value": { "battery": { "percent": 50, "override": true } },
                        "source": "discovered"
                    });
                }
            }
            let payload = serde_json::to_vec(&value).unwrap();
            let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
            frame.extend(payload);
            assert!(
                decode_control_frame(&frame).is_err(),
                "accepted case {path}"
            );
        }
    }

    #[test]
    fn invalid_claimed_capacity_and_battery_percent_fail_closed() {
        let mut envelope = sample();
        let ControlMessage::Hello { facts, .. } = &mut envelope.message else {
            unreachable!()
        };
        facts.capabilities.connection_policy_ceiling = Some(blueroute_core::Sourced::new(
            0,
            blueroute_core::CapabilitySource::Discovered,
        ));
        assert!(encode_control_frame(&envelope).is_err());
        let ControlMessage::Hello { facts, .. } = &mut envelope.message else {
            unreachable!()
        };
        facts.capabilities.connection_policy_ceiling = None;
        facts.capabilities.power_state = Some(blueroute_core::Sourced::new(
            blueroute_core::PowerState::Battery { percent: 101 },
            blueroute_core::CapabilitySource::Discovered,
        ));
        assert!(encode_control_frame(&envelope).is_err());
    }

    #[test]
    fn per_session_replay_and_identity_checks() {
        let first = sample();
        let mut tracker =
            VerifiedSessionSequence::new(first.network, first.sender, first.session_id).unwrap();
        tracker.accept(&first).unwrap();
        assert!(tracker.accept(&first).is_err());
        let mut next = first.clone();
        next.sequence = 2;
        next.sender = NodeId::from_bytes([8; 16]);
        assert!(tracker.accept(&next).is_err());
        next.sender = first.sender;
        next.network = NetworkId::from_bytes([8; 16]);
        assert!(tracker.accept(&next).is_err());
        next.network = first.network;
        next.session_id = [8; 16];
        assert!(tracker.accept(&next).is_err());
        next.session_id = first.session_id;
        tracker.accept(&next).unwrap();
        assert_eq!(tracker.last_sequence(), 2);
        assert!(tracker.accept(&first).is_err());
    }

    #[test]
    fn arbitrary_short_bytes_are_rejected_without_panicking() {
        let mut state = 0x1234_5678_u32;
        for size in 0..384_usize {
            let mut frame = vec![0_u8; size];
            for byte in &mut frame {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            let _ = decode_control_frame(&frame);
        }
    }

    #[test]
    fn hello_carries_capabilities_segment_and_peer_facts() {
        let mut message = sample();
        let ControlMessage::Hello { facts, .. } = &mut message.message else {
            panic!("expected hello");
        };
        facts.capabilities.panu = Some(blueroute_core::Sourced::new(
            true,
            blueroute_core::CapabilitySource::Measured,
        ));
        facts.segment = Some(IpPrefix::new("10.201.44.0".parse().unwrap(), 24).unwrap());
        let encoded = encode_control_frame(&message).unwrap();
        assert_eq!(decode_control_frame(&encoded).unwrap(), message);
    }

    #[test]
    fn rejects_forged_and_malformed_hello_facts() {
        for case in 0..6 {
            let mut message = sample();
            let ControlMessage::Hello { facts, .. } = &mut message.message else {
                panic!("expected hello");
            };
            match case {
                0 => facts.software_version = " ".into(),
                1 => facts.software_version = "x".repeat(65),
                2 => facts.direct_neighbors.push(NodeId::from_bytes([7; 16])),
                3 => facts.direct_neighbors.push(NodeId::from_bytes([3; 16])),
                4 => {
                    facts.segment =
                        Some(IpPrefix::new("10.201.44.9".parse().unwrap(), 24).unwrap());
                }
                5 => {
                    facts.capabilities.link_quality = Some(blueroute_core::Sourced::new(
                        blueroute_core::LinkQuality::new(101),
                        blueroute_core::CapabilitySource::Discovered,
                    ));
                }
                _ => unreachable!(),
            }
            assert!(encode_control_frame(&message).is_err());
        }
    }

    #[test]
    fn rejects_unknown_hello_fields() {
        let data =
            String::from_utf8(encode_control_frame(&sample()).unwrap()[4..].to_vec()).unwrap();
        let altered = data.replace(
            "\"direct_neighbors\":",
            "\"unknown_required_feature\":true,\"direct_neighbors\":",
        );
        let mut frame = (altered.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(altered.as_bytes());
        assert!(decode_control_frame(&frame).is_err());
    }

    #[test]
    fn all_initial_message_variants_round_trip() {
        let baseline = sample();
        let facts = example_facts();
        let messages = [
            ControlMessage::Hello {
                challenge: [5; 16],
                facts: facts.clone(),
            },
            ControlMessage::HelloAck {
                challenge: [7; 16],
                facts,
            },
            ControlMessage::Heartbeat,
            ControlMessage::Goodbye,
        ];
        for (index, message) in messages.into_iter().enumerate() {
            let envelope = ControlEnvelope {
                sequence: index as u64 + 1,
                message,
                ..baseline.clone()
            };
            let encoded = encode_control_frame(&envelope).unwrap();
            assert_eq!(decode_control_frame(&encoded).unwrap(), envelope);
        }
    }

    #[test]
    fn known_frames_with_mutated_prefixes_and_versions_fail_closed() {
        let valid = encode_control_frame(&sample()).unwrap();
        let length = valid.len() - 4;
        for declared in [
            0_usize,
            1,
            length - 1,
            length + 1,
            MAX_CONTROL_FRAME_BYTES + 1,
        ] {
            let mut wrong = valid.clone();
            wrong[..4].copy_from_slice(&(declared as u32).to_be_bytes());
            assert!(decode_control_frame(&wrong).is_err());
        }
        let raw = String::from_utf8(valid[4..].to_vec()).unwrap();
        let downgraded = raw.replace("\"version\":1", "\"version\":0");
        let future = raw.replace("\"version\":1", "\"version\":2");
        for payload in [downgraded, future] {
            let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
            framed.extend_from_slice(payload.as_bytes());
            assert!(decode_control_frame(&framed).is_err());
        }
    }

    #[test]
    fn frame_mutations_and_all_truncated_valid_prefixes_never_panic() {
        let valid = encode_control_frame(&sample()).unwrap();
        for length in 0..valid.len() {
            assert!(decode_control_frame(&valid[..length]).is_err());
        }
        for index in 0..valid.len() {
            for alteration in [0_u8, 0xff] {
                let mut altered = valid.clone();
                altered[index] = alteration;
                let _ = decode_control_frame(&altered);
            }
        }
    }
}

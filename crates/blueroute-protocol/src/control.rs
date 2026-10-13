//! Untrusted, bounded inter-node control-plane framing (not the local D-Bus API).
//!
//! A decoded message is NOT an authenticated peer. A TLS session must verify
//! the remote cryptographic identity and bind the expected NodeId, NetworkId
//! and session nonce to the channel before this data may authorize anything.

use std::fmt;

use blueroute_core::{NetworkId, NodeId};
use serde::{Deserialize, Serialize};

/// Versioned independently from the local daemon API.
pub const CONTROL_VERSION: u16 = 1;
/// Maximum number of bytes in a serialized control payload.
pub const MAX_CONTROL_FRAME_BYTES: usize = 16 * 1024;
const HEADER_LEN: usize = 4;

/// Initial bootstrap and liveness types only; no unauthenticated route changes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ControlMessage {
    Hello { challenge: [u8; 16] },
    HelloAck { challenge: [u8; 16] },
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
        ControlMessage::Hello { challenge } | ControlMessage::HelloAck { challenge }
            if challenge.iter().all(|byte| *byte == 0) =>
        {
            return Err(invalid("zero hello challenge"));
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

    fn sample() -> ControlEnvelope {
        ControlEnvelope {
            version: CONTROL_VERSION,
            network: NetworkId::from_bytes([2; 16]),
            sender: NodeId::from_bytes([3; 16]),
            session_id: [4; 16],
            sequence: 1,
            message: ControlMessage::Hello { challenge: [5; 16] },
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
        envelope.message = ControlMessage::Hello { challenge: [0; 16] };
        assert!(encode_control_frame(&envelope).is_err());
    }

    #[test]
    fn unknown_types_and_critical_fields_fail_closed() {
        let encoded = encode_control_frame(&sample()).unwrap();
        let json = String::from_utf8(encoded[4..].to_vec()).unwrap();
        let bad_type = json.replace("\"hello\"", "\"unauthorized_route\"");
        let bad_field = json.replace("\"sequence\":1,", "\"sequence\":1,\"superuser\":true,");
        for payload in [bad_type, bad_field] {
            let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
            framed.extend_from_slice(payload.as_bytes());
            assert!(decode_control_frame(&framed).is_err());
        }
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
}

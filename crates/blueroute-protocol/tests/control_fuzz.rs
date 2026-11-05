//! Malformed-input, round-trip, and compatibility tests for the control parser (P7-005).
//!
//! A seeded xorshift generator keeps the cases deterministic without extra dependencies.
//! The invariant under test: no peer-supplied byte sequence may panic the decoder, the
//! validator, or the hello parser.

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::{
    CONTROL_FRAME_HEADER_BYTES, ControlEnvelope, ControlError, ControlVersion, Inbound,
    InboundValidator, MAX_CONTROL_FRAME_BYTES, MessageType, decode_frame, encode_frame,
    frame_body_len,
};
use blueroute_protocol::hello::Hello;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max + 1);
        (0..len).map(|_| self.next() as u8).collect()
    }
}

fn network() -> NetworkId {
    NetworkId::from_bytes([1; 16])
}

fn peer() -> NodeId {
    NodeId::from_bytes([2; 16])
}

fn sample(sequence: u64, kind: &str, payload: serde_json::Value) -> ControlEnvelope {
    ControlEnvelope {
        version: ControlVersion::CURRENT,
        network: network(),
        sender: peer(),
        sequence,
        message_type: MessageType::new(kind).unwrap(),
        critical: false,
        payload,
    }
}

#[test]
fn random_bytes_never_panic_the_frame_decoder() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let data = rng.bytes(256);
        let _ = frame_body_len(&data);
        let _ = decode_frame(&data);
    }
}

#[test]
fn random_bodies_with_a_valid_header_never_panic() {
    let mut rng = Rng(42);
    for _ in 0..10_000 {
        let body = rng.bytes(512);
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        let _ = decode_frame(&frame);
    }
}

#[test]
fn mutated_valid_frames_never_panic() {
    let mut rng = Rng(7);
    let good = encode_frame(&sample(1, "hello", serde_json::json!({"node": "x"}))).unwrap();
    for _ in 0..20_000 {
        let mut frame = good.clone();
        for _ in 0..=(rng.next() % 4) {
            let i = (rng.next() as usize) % frame.len();
            frame[i] = rng.next() as u8;
        }
        if rng.next().is_multiple_of(3) {
            frame.truncate((rng.next() as usize) % (frame.len() + 1));
        }
        if let Ok(envelope) = decode_frame(&frame) {
            let mut validator = InboundValidator::new(network(), peer());
            let _ = validator.accept(envelope.clone());
            let _ = Hello::from_envelope(&envelope);
        }
    }
}

#[test]
fn oversized_declared_lengths_are_rejected_before_buffering() {
    for declared in [MAX_CONTROL_FRAME_BYTES as u32 + 1, u32::MAX, 0x8000_0000] {
        let header = declared.to_be_bytes();
        assert!(matches!(
            frame_body_len(&header),
            Err(ControlError::Oversized { .. })
        ));
    }
    assert_eq!(frame_body_len(&[0, 0]), Err(ControlError::Truncated));
}

#[test]
fn round_trip_holds_for_varied_envelopes() {
    let mut rng = Rng(99);
    for i in 0..500u64 {
        let payload = serde_json::json!({
            "n": rng.next(),
            "s": format!("v{}", rng.next() % 1000),
            "list": [i, rng.next() % 10],
        });
        let mut envelope = sample(rng.next() >> 1, "hello", payload);
        envelope.critical = i.is_multiple_of(2);
        let frame = encode_frame(&envelope).unwrap();
        assert_eq!(decode_frame(&frame).unwrap(), envelope);
        assert_eq!(
            frame_body_len(&frame).unwrap(),
            frame.len() - CONTROL_FRAME_HEADER_BYTES
        );
    }
}

#[test]
fn compatibility_newer_minor_and_unknown_fields_decode() {
    // A newer peer adds an unknown envelope field and an unknown non-critical type.
    let body = serde_json::json!({
        "version": {"major": 1, "minor": 7},
        "network": network().to_string(),
        "sender": peer().to_string(),
        "sequence": 5,
        "message_type": "future.thing",
        "payload": {"anything": [1, 2, 3]},
        "added_in_minor_7": true
    });
    let body = serde_json::to_vec(&body).unwrap();
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    let envelope = decode_frame(&frame).expect("unknown fields and higher minors are tolerated");
    let mut validator = InboundValidator::new(network(), peer());
    assert!(matches!(
        validator.accept(envelope),
        Ok(Inbound::Ignored { .. })
    ));
}

#[test]
fn compatibility_major_bump_and_unknown_critical_are_rejected() {
    let mut validator = InboundValidator::new(network(), peer());
    let mut future = sample(1, "hello", serde_json::json!({}));
    future.version.major = 2;
    assert!(matches!(
        validator.accept(future),
        Err(ControlError::UnsupportedMajorVersion { received: 2 })
    ));
    let mut critical = sample(2, "future.thing", serde_json::json!({}));
    critical.critical = true;
    assert!(matches!(
        validator.accept(critical),
        Err(ControlError::UnknownCritical(_))
    ));
}

#[test]
fn random_validator_sequences_never_panic_and_stay_strictly_increasing() {
    let mut rng = Rng(1234);
    let mut validator = InboundValidator::new(network(), peer());
    let mut last: Option<u64> = None;
    for _ in 0..10_000 {
        let sequence = rng.next() % 50;
        let accepted = validator
            .accept(sample(sequence, "hello", serde_json::json!({})))
            .is_ok();
        if accepted {
            if let Some(prev) = last {
                assert!(sequence > prev);
            }
            last = Some(sequence);
        } else if let Some(prev) = last {
            assert!(sequence <= prev);
        }
    }
}

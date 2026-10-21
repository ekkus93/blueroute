//! Negative control-wire tests: duplicate JSON names must never select an
//! attacker-controlled interpretation of logical or authenticated identity.
//! These inputs are untrusted claims, not TLS or Bluetooth acceptance evidence.

use blueroute_core::{NetworkId, NodeCapabilities, NodeId};
use blueroute_protocol::control::{
    CONTROL_VERSION, ControlEnvelope, ControlMessage, HelloFacts, decode_control_frame,
    encode_control_frame,
};
use serde_json::Value;

fn sample(message: ControlMessage) -> ControlEnvelope {
    ControlEnvelope {
        version: CONTROL_VERSION,
        network: NetworkId::from_bytes([2; 16]),
        sender: NodeId::from_bytes([3; 16]),
        session_id: [4; 16],
        sequence: 1,
        message,
    }
}

fn hello() -> ControlEnvelope {
    sample(ControlMessage::Hello {
        challenge: [5; 16],
        facts: HelloFacts {
            software_version: "1.0.0".to_owned(),
            capabilities: NodeCapabilities::default(),
            segment: None,
            direct_neighbors: vec![NodeId::from_bytes([7; 16])],
        },
    })
}

fn raw(envelope: &ControlEnvelope) -> String {
    let encoded = encode_control_frame(envelope).expect("baseline frame");
    String::from_utf8(encoded[4..].to_vec()).expect("JSON payload")
}

fn frame(payload: &str) -> Vec<u8> {
    let mut framed = u32::try_from(payload.len())
        .expect("small test frame")
        .to_be_bytes()
        .to_vec();
    framed.extend_from_slice(payload.as_bytes());
    framed
}

fn add_duplicate(payload: &str, key: &str, value: &Value) -> String {
    let marker = format!("\"{key}\":");
    assert!(
        payload.contains(&marker),
        "fixture is missing the {key} field"
    );
    payload.replacen(
        &marker,
        &format!("{marker}{value},{marker}"),
        1,
    )
}

#[test]
fn duplicate_root_identity_and_session_claims_are_rejected() {
    let baseline = sample(ControlMessage::Heartbeat);
    let payload = raw(&baseline);
    let decoded: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(decode_control_frame(&frame(&payload)).unwrap(), baseline);

    for field in [
        "version",
        "network",
        "sender",
        "session_id",
        "sequence",
        "message",
    ] {
        let hostile = add_duplicate(&payload, field, &decoded[field]);
        assert!(
            decode_control_frame(&frame(&hostile)).is_err(),
            "accepted repeated top-level control field {field}"
        );
    }
}

#[test]
fn duplicate_hello_challenge_and_nested_claims_are_rejected() {
    let baseline = hello();
    let payload = raw(&baseline);
    let decoded: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(decode_control_frame(&frame(&payload)).unwrap(), baseline);

    let message = &decoded["message"]["data"];
    let facts = &message["facts"];
    let capability = &facts["capabilities"];

    for (field, value) in [
        ("challenge", &message["challenge"]),
        ("facts", facts),
        ("software_version", &facts["software_version"]),
        ("capabilities", capability),
        ("direct_neighbors", &facts["direct_neighbors"]),
        ("segment", &facts["segment"]),
        ("nap", &capability["nap"]),
    ] {
        let hostile = add_duplicate(&payload, field, value);
        assert!(
            decode_control_frame(&frame(&hostile)).is_err(),
            "accepted repeated Hello or nested-capability field {field}"
        );
    }
}

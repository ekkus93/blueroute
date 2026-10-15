//! P7-005 hostile inbound-stream regression coverage for the P7-004 idle gate.
//!
//! These tests provide no cryptographic proof: the runtime model assumes an
//! independently verified mutual-TLS channel and pinned peer identity. Never
//! interpret their passing as authenticated production JoinNetwork acceptance.

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::{
    CONTROL_VERSION, ControlEnvelope, ControlMessage, encode_control_frame,
};
use blueroute_protocol::session::{ConnectionDeadline, ConnectionStage, DeadlineError};
use blueroute_protocol::session_activity::{AuthenticatedInboundActivity, InboundActivityError};

fn network() -> NetworkId {
    NetworkId::from_bytes([31; 16])
}

fn peer(value: u8) -> NodeId {
    NodeId::from_bytes([value; 16])
}

fn active_deadline() -> ConnectionDeadline {
    let mut deadline = ConnectionDeadline::new(0);
    deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
    deadline.advance(ConnectionStage::Hello, 2).unwrap();
    deadline.advance(ConnectionStage::Active, 3).unwrap();
    deadline
}

fn activity(remote: NodeId) -> AuthenticatedInboundActivity {
    AuthenticatedInboundActivity::after_verified_hello(
        network(),
        remote,
        [49; 16],
        active_deadline(),
    )
    .unwrap()
}

fn frame(sender: NodeId, seq: u64, message: ControlMessage) -> Vec<u8> {
    encode_control_frame(&ControlEnvelope {
        version: CONTROL_VERSION,
        network: network(),
        sender,
        session_id: [49; 16],
        sequence: seq,
        message,
    })
    .unwrap()
}

#[test]
fn all_heartbeat_fragment_boundaries_obey_original_idle_deadline() {
    let wire = frame(peer(51), 3, ControlMessage::Heartbeat);
    for split in 1..wire.len() {
        let mut inbound = activity(peer(51));
        assert!(
            inbound
                .feed_verified_plaintext(&wire[..split], 29_999)
                .unwrap()
                .is_empty()
        );
        assert_eq!(inbound.expires_ms(), 30_003);
        assert_eq!(
            inbound.feed_verified_plaintext(&wire[split..], 30_003),
            Err(InboundActivityError::Deadline(DeadlineError::Expired))
        );
        assert!(inbound.is_terminal());
        assert_eq!(
            inbound.feed_verified_plaintext(&wire, 30_004),
            Err(InboundActivityError::Terminal)
        );
    }
}

#[test]
fn validated_peer_activity_does_not_extend_another_peer_lifetime() {
    let mut alice = activity(peer(51));
    let mut bob = activity(peer(52));
    let from_bob = frame(peer(52), 3, ControlMessage::Heartbeat);
    let from_alice = frame(peer(51), 3, ControlMessage::Heartbeat);

    assert_eq!(
        alice.feed_verified_plaintext(&from_bob, 10),
        Err(InboundActivityError::Scope(
            blueroute_protocol::session_stream::SessionStreamError::WrongContext
        ))
    );
    assert!(alice.is_terminal());
    assert_eq!(bob.feed_verified_plaintext(&from_bob, 20).unwrap().len(), 1);
    assert_eq!(bob.expires_ms(), 30_020);
    assert_eq!(
        alice.feed_verified_plaintext(&from_alice, 21),
        Err(InboundActivityError::Terminal)
    );
    assert_eq!(
        bob.feed_verified_plaintext(&from_bob, 22),
        Err(InboundActivityError::Scope(
            blueroute_protocol::session_stream::SessionStreamError::Replay
        ))
    );
    assert!(bob.is_terminal());
}

#[test]
fn two_thousand_forty_eight_reproducible_wire_mutations_cannot_bypass_scope() {
    let original = frame(peer(51), 3, ControlMessage::Heartbeat);
    let mut seed = 0x5a17_c0de_u32;
    for i in 0..2048 {
        let mut input = original.clone();
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let index = (seed as usize) % input.len();
        let xor = (1_u8 << (i % 8)) | 1;
        input[index] ^= xor;

        let mut inbound = activity(peer(51));
        let result = inbound.feed_verified_plaintext(&input, 100);
        match result {
            Ok(frames) => {
                // A mutation may still encode a valid, in-scope heartbeat.
                // It can refresh liveness only after complete strict decoding.
                if !frames.is_empty() {
                    assert_eq!(frames.len(), 1);
                    let accepted = &frames[0];
                    assert_eq!(accepted.network, network());
                    assert_eq!(accepted.sender, peer(51));
                    assert_eq!(accepted.session_id, [49; 16]);
                    assert!(accepted.sequence > 2);
                    assert!(matches!(accepted.message, ControlMessage::Heartbeat));
                    assert_eq!(inbound.expires_ms(), 30_100);
                } else {
                    assert_eq!(inbound.expires_ms(), 30_003);
                }
            }
            Err(_) => {
                assert!(inbound.is_terminal());
                assert_eq!(
                    inbound.feed_verified_plaintext(&original, 101),
                    Err(InboundActivityError::Terminal)
                );
            }
        }
    }
}

#[test]
fn full_heartbeat_without_goodbye_is_never_a_graceful_disconnect() {
    let mut inbound = activity(peer(51));
    let heartbeat = frame(peer(51), 3, ControlMessage::Heartbeat);
    assert_eq!(
        inbound
            .feed_verified_plaintext(&heartbeat, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(inbound.finish_at(11).is_err());
    assert!(inbound.is_terminal());
    assert_eq!(
        inbound.feed_verified_plaintext(&heartbeat, 12),
        Err(InboundActivityError::Terminal)
    );
}

//! P7-005 deterministic malicious batch tests for the P7-004 idle gate.
//!
//! The parser accepts plaintext only after a separate authenticated TLS
//! handshake and pinned identity check. These tests do NOT establish TLS
//! authentication, PAN transport, or production JoinNetwork acceptance.

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::{
    CONTROL_VERSION, ControlEnvelope, ControlMessage, encode_control_frame,
};
use blueroute_protocol::control_stream::StreamDecodeError;
use blueroute_protocol::session::{ConnectionDeadline, ConnectionStage};
use blueroute_protocol::session_activity::{AuthenticatedInboundActivity, InboundActivityError};
use blueroute_protocol::session_stream::SessionStreamError;

fn network() -> NetworkId {
    NetworkId::from_bytes([71; 16])
}

fn peer(value: u8) -> NodeId {
    NodeId::from_bytes([value; 16])
}

fn active_session() -> AuthenticatedInboundActivity {
    let mut deadline = ConnectionDeadline::new(0);
    deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
    deadline.advance(ConnectionStage::Hello, 2).unwrap();
    deadline.advance(ConnectionStage::Active, 3).unwrap();
    AuthenticatedInboundActivity::after_verified_hello(network(), peer(72), [73; 16], deadline)
        .unwrap()
}

fn frame(sender: NodeId, sequence: u64, message: ControlMessage) -> Vec<u8> {
    encode_control_frame(&ControlEnvelope {
        version: CONTROL_VERSION,
        network: network(),
        sender,
        session_id: [73; 16],
        sequence,
        message,
    })
    .unwrap()
}

#[test]
fn valid_heartbeat_followed_by_wrong_peer_in_one_batch_never_renews_idle() {
    let mut session = active_session();
    let mut batch = frame(peer(72), 3, ControlMessage::Heartbeat);
    batch.extend(frame(peer(74), 4, ControlMessage::Heartbeat));
    assert_eq!(
        session.feed_verified_plaintext(&batch, 100),
        Err(InboundActivityError::Scope(
            SessionStreamError::WrongContext
        ))
    );
    assert_eq!(session.expires_ms(), 30_003);
    assert!(session.is_terminal());
    assert_eq!(
        session.feed_verified_plaintext(&frame(peer(72), 5, ControlMessage::Heartbeat), 101),
        Err(InboundActivityError::Terminal)
    );
}

#[test]
fn valid_heartbeat_followed_by_replay_in_one_batch_never_renews_idle() {
    let mut session = active_session();
    let heartbeat = frame(peer(72), 3, ControlMessage::Heartbeat);
    let mut batch = heartbeat.clone();
    batch.extend(heartbeat);
    assert_eq!(
        session.feed_verified_plaintext(&batch, 100),
        Err(InboundActivityError::Scope(SessionStreamError::Replay))
    );
    assert_eq!(session.expires_ms(), 30_003);
    assert!(session.is_terminal());
}

#[test]
fn goodbye_with_trailing_incomplete_header_is_not_graceful() {
    let mut session = active_session();
    let mut batch = frame(peer(72), 3, ControlMessage::Goodbye);
    batch.push(0);
    assert_eq!(
        session.feed_verified_plaintext(&batch, 100),
        Err(InboundActivityError::Scope(
            SessionStreamError::AfterGoodbye
        ))
    );
    assert!(!session.goodbye_seen());
    assert!(session.is_terminal());
    assert_eq!(session.finish_at(101), Err(InboundActivityError::Terminal));
}

#[test]
fn valid_coalesced_messages_renew_idle_only_after_whole_batch_validation() {
    let mut session = active_session();
    let mut batch = frame(peer(72), 3, ControlMessage::Heartbeat);
    batch.extend(frame(peer(72), 4, ControlMessage::Heartbeat));
    let accepted = session.feed_verified_plaintext(&batch, 100).unwrap();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].sequence, 3);
    assert_eq!(accepted[1].sequence, 4);
    assert_eq!(session.expires_ms(), 30_100);
    assert!(!session.is_terminal());
}

#[test]
fn oversized_coalesced_frame_flood_is_atomic_and_terminal() {
    let mut session = active_session();
    let mut batch = Vec::new();
    for sequence in 3..=67 {
        batch.extend(frame(peer(72), sequence, ControlMessage::Heartbeat));
    }
    assert_eq!(
        session.feed_verified_plaintext(&batch, 100),
        Err(InboundActivityError::Scope(SessionStreamError::Decode(
            StreamDecodeError::FrameLimit
        )))
    );
    assert_eq!(session.expires_ms(), 30_003);
    assert!(session.is_terminal());
}

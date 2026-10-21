//! P7-005 deterministic adversarial fuzz-style protocol tests.
//!
//! Simulated verified-session inputs only: this test does not perform TLS,
//! authenticate a peer, grant BlueRoute membership, or qualify Bluetooth.

use std::panic::catch_unwind;

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::{
    CONTROL_VERSION, ControlEnvelope, ControlMessage, encode_control_frame,
};
use blueroute_protocol::session_stream::{SessionControlStream, SessionStreamError};

fn network() -> NetworkId {
    NetworkId::from_bytes([1; 16])
}

fn peer() -> NodeId {
    NodeId::from_bytes([2; 16])
}

fn frame(sequence: u64, message: ControlMessage) -> ControlEnvelope {
    ControlEnvelope {
        version: CONTROL_VERSION,
        network: network(),
        sender: peer(),
        session_id: [3; 16],
        sequence,
        message,
    }
}

fn stream() -> SessionControlStream {
    // This provides a fixture context, not real cryptographic authentication.
    SessionControlStream::after_verified_hello(network(), peer(), [3; 16]).unwrap()
}

#[test]
fn all_chunk_boundaries_preserve_valid_order_until_goodbye() {
    let expected = [
        frame(3, ControlMessage::Heartbeat),
        frame(4, ControlMessage::Heartbeat),
        frame(5, ControlMessage::Goodbye),
    ];
    let mut wire = Vec::new();
    for envelope in &expected {
        wire.extend_from_slice(&encode_control_frame(envelope).unwrap());
    }
    for width in [1_usize, 2, 3, 4, 7, 13, 31, 128, wire.len()] {
        let mut session = stream();
        let mut decoded = Vec::new();
        for chunk in wire.chunks(width) {
            decoded.extend(session.feed(chunk).unwrap());
        }
        assert_eq!(decoded, expected, "chunk width={width}");
        assert_eq!(session.last_sequence(), 5);
        assert!(session.is_closed());
        assert_eq!(session.finish(), Ok(()));
        assert_eq!(
            session.feed(&encode_control_frame(&expected[0]).unwrap()),
            Err(SessionStreamError::Poisoned)
        );
    }
}

#[test]
fn deterministic_mutations_never_panic_or_recover_from_rejection() {
    let originals = [
        frame(3, ControlMessage::Heartbeat),
        frame(4, ControlMessage::Heartbeat),
        frame(5, ControlMessage::Goodbye),
    ];
    let mut valid = Vec::new();
    for envelope in &originals {
        valid.extend_from_slice(&encode_control_frame(envelope).unwrap());
    }
    let mut seed = 0x2ce7_85b9_d5ca_1e13_u64;
    for iteration in 0..2048_usize {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mut wire = valid.clone();
        let index = (seed as usize) % wire.len();
        wire[index] ^= ((seed >> 40) as u8) | 1;
        if iteration % 5 == 0 {
            wire.truncate(((seed >> 8) as usize) % wire.len());
        }
        if iteration % 11 == 0 {
            wire.extend_from_slice(&[0, 0, 0, 0]);
        }
        let width = 1 + (((seed >> 16) as usize) % 37);
        let result = catch_unwind(|| {
            let mut session = stream();
            let mut rejected = false;
            for chunk in wire.chunks(width) {
                if session.feed(chunk).is_err() {
                    rejected = true;
                    break;
                }
            }
            if rejected {
                assert_eq!(
                    session.feed(&encode_control_frame(&originals[0]).unwrap()),
                    Err(SessionStreamError::Poisoned)
                );
            } else {
                // A valid mutation can stay syntactically valid; EOF is
                // checked separately from frame parsing and replay checks.
                let _ = session.finish();
            }
        });
        assert!(
            result.is_ok(),
            "control stream panicked at mutation {iteration}"
        );
    }
}

#[test]
fn every_truncated_valid_transcript_fails_closed_at_eof() {
    // Exhaustively exercise every EOF position, including exact boundaries
    // between otherwise valid frames. Only a complete Goodbye permits a
    // graceful EOF; incomplete data must permanently retire the parser.
    let mut wire = Vec::new();
    for envelope in [
        frame(3, ControlMessage::Heartbeat),
        frame(4, ControlMessage::Heartbeat),
        frame(5, ControlMessage::Goodbye),
    ] {
        wire.extend_from_slice(&encode_control_frame(&envelope).unwrap());
    }

    for cut in 0..wire.len() {
        for width in [1_usize, 3, 17, wire.len()] {
            let mut session = stream();
            for chunk in wire[..cut].chunks(width) {
                session.feed(chunk).unwrap();
            }
            assert!(
                session.finish().is_err(),
                "truncated transcript accepted at byte {cut}, chunk width {width}"
            );
            assert_eq!(
                session.feed(&wire),
                Err(SessionStreamError::Poisoned),
                "parser recovered after truncated EOF at byte {cut}"
            );
        }
    }

    let mut complete = stream();
    for chunk in wire.chunks(3) {
        complete.feed(chunk).unwrap();
    }
    assert_eq!(complete.finish(), Ok(()));
    assert_eq!(complete.feed(&wire), Err(SessionStreamError::Poisoned));
}

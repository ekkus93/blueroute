//! Post-hello control-stream scope and replay checks.
//!
//! This is a security boundary only for *already* verified connections. It
//! does NOT perform TLS, verify a certificate, enroll a key, or authenticate
//! a peer. Never construct it from an untrusted Hello or a raw TCP socket.
//! Production JoinNetwork must remain disabled until mutual TLS 1.3, pin
//! approval, and channel binding have been implemented by the daemon.

use blueroute_core::{NetworkId, NodeId};

use crate::control::{ControlEnvelope, ControlMessage};
use crate::control_stream::{ControlStreamDecoder, StreamDecodeError};

/// Bounded parsing and context checks for frames after both Hello/HelloAck
/// records have been validated on a *separately authenticated* TLS channel.
/// Inbound sequences 1 and 2 were consumed during that exchange.
#[derive(Debug)]
pub struct SessionControlStream {
    decoder: ControlStreamDecoder,
    network: NetworkId,
    peer: NodeId,
    exporter_session_id: [u8; 16],
    last_sequence: u64,
    closed: bool,
    poisoned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStreamError {
    InvalidSession,
    Decode(StreamDecodeError),
    WrongContext,
    Replay,
    UnexpectedHandshake,
    AfterGoodbye,
    Poisoned,
}

impl SessionControlStream {
    /// Caller MUST have already authenticated the TLS peer key against a
    /// durable user-approved (NetworkId, NodeId) pin and checked the complete
    /// Hello transcript against this exact TLS exporter output.
    pub fn after_verified_hello(
        network: NetworkId,
        peer: NodeId,
        exporter_session_id: [u8; 16],
    ) -> Result<Self, SessionStreamError> {
        if exporter_session_id == [0; 16] {
            return Err(SessionStreamError::InvalidSession);
        }
        Ok(Self {
            decoder: ControlStreamDecoder::new(),
            network,
            peer,
            exporter_session_id,
            last_sequence: 2,
            closed: false,
            poisoned: false,
        })
    }

    /// Parse a bounded TLS plaintext chunk. No frames are returned if *any*
    /// frame in the chunk fails validation, including a late malformed frame
    /// after earlier valid ones. The entire session is then irrecoverable.
    pub fn feed(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<ControlEnvelope>, SessionStreamError> {
        if self.poisoned {
            return Err(SessionStreamError::Poisoned);
        }
        if self.closed {
            if !bytes.is_empty() {
                return self.reject(SessionStreamError::AfterGoodbye);
            }
            return Ok(Vec::new());
        }
        let frames = match self.decoder.feed(bytes) {
            Ok(frames) => frames,
            Err(error) => return self.reject(SessionStreamError::Decode(error)),
        };
        for frame in &frames {
            if frame.network != self.network
                || frame.sender != self.peer
                || frame.session_id != self.exporter_session_id
            {
                return self.reject(SessionStreamError::WrongContext);
            }
            if frame.sequence <= self.last_sequence {
                return self.reject(SessionStreamError::Replay);
            }
            if self.closed {
                return self.reject(SessionStreamError::AfterGoodbye);
            }
            match frame.message {
                ControlMessage::Heartbeat => {}
                ControlMessage::Goodbye => self.closed = true,
                ControlMessage::Hello { .. } | ControlMessage::HelloAck { .. } => {
                    return self.reject(SessionStreamError::UnexpectedHandshake);
                }
            }
            self.last_sequence = frame.sequence;
        }
        Ok(frames)
    }

    /// EOF is clean only at a frame boundary. An incomplete frame poisons the
    /// session, and a later feed cannot resynchronize it.
    pub fn finish(&mut self) -> Result<(), SessionStreamError> {
        if self.poisoned {
            return Err(SessionStreamError::Poisoned);
        }
        match self.decoder.finish() {
            Ok(()) => Ok(()),
            Err(error) => self.reject(SessionStreamError::Decode(error)),
        }
    }

    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    fn reject<T>(&mut self, error: SessionStreamError) -> Result<T, SessionStreamError> {
        self.poisoned = true;
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{CONTROL_VERSION, HelloFacts, encode_control_frame};
    use blueroute_core::NodeCapabilities;

    fn network() -> NetworkId {
        NetworkId::from_bytes([1; 16])
    }

    fn peer() -> NodeId {
        NodeId::from_bytes([2; 16])
    }

    fn message(sequence: u64, message: ControlMessage) -> ControlEnvelope {
        ControlEnvelope {
            version: CONTROL_VERSION,
            network: network(),
            sender: peer(),
            session_id: [3; 16],
            sequence,
            message,
        }
    }

    fn heartbeat(sequence: u64) -> ControlEnvelope {
        message(sequence, ControlMessage::Heartbeat)
    }

    fn stream() -> SessionControlStream {
        SessionControlStream::after_verified_hello(network(), peer(), [3; 16]).unwrap()
    }

    fn bytes(frame: &ControlEnvelope) -> Vec<u8> {
        encode_control_frame(frame).unwrap()
    }

    #[test]
    fn fragments_are_checked_after_complete_frames_and_goodbye_terminates() {
        let mut session = stream();
        let frame = heartbeat(3);
        let encoded = bytes(&frame);
        for part in encoded[..encoded.len() - 1].chunks(1) {
            assert!(session.feed(part).unwrap().is_empty());
            assert_eq!(session.last_sequence(), 2);
        }
        assert_eq!(session.feed(&encoded[encoded.len() - 1..]).unwrap(), vec![frame]);
        assert_eq!(session.last_sequence(), 3);
        let goodbye = message(4, ControlMessage::Goodbye);
        assert_eq!(session.feed(&bytes(&goodbye)).unwrap(), vec![goodbye]);
        assert!(session.is_closed());
        assert_eq!(session.finish(), Ok(()));
        assert_eq!(session.feed(&bytes(&heartbeat(5))), Err(SessionStreamError::AfterGoodbye));
        assert_eq!(session.feed(&[]), Err(SessionStreamError::Poisoned));
    }

    #[test]
    fn wrong_peer_network_exporter_and_handshake_replay_fail_closed() {
        let mut wrong_network = heartbeat(3);
        wrong_network.network = NetworkId::from_bytes([8; 16]);
        let mut wrong_peer = heartbeat(3);
        wrong_peer.sender = NodeId::from_bytes([8; 16]);
        let mut wrong_exporter = heartbeat(3);
        wrong_exporter.session_id = [8; 16];
        let stale_handshake = heartbeat(2);
        let later_hello = message(
            3,
            ControlMessage::Hello {
                challenge: [7; 16],
                facts: HelloFacts {
                    software_version: "0.1.0".to_owned(),
                    capabilities: NodeCapabilities::default(),
                    segment: None,
                    direct_neighbors: Vec::new(),
                },
            },
        );
        for (frame, expected) in [
            (wrong_network, SessionStreamError::WrongContext),
            (wrong_peer, SessionStreamError::WrongContext),
            (wrong_exporter, SessionStreamError::WrongContext),
            (stale_handshake, SessionStreamError::Replay),
            (later_hello, SessionStreamError::UnexpectedHandshake),
        ] {
            let mut session = stream();
            assert_eq!(session.feed(&bytes(&frame)), Err(expected));
            assert_eq!(session.feed(&bytes(&heartbeat(3))), Err(SessionStreamError::Poisoned));
        }
        assert!(matches!(
            SessionControlStream::after_verified_hello(network(), peer(), [0; 16]),
            Err(SessionStreamError::InvalidSession)
        ));
    }

    #[test]
    fn replay_out_of_order_and_after_goodbye_are_rejected_atomically() {
        for invalid in [heartbeat(3), heartbeat(2)] {
            let mut session = stream();
            let mut combined = bytes(&heartbeat(3));
            combined.extend(bytes(&invalid));
            assert!(session.feed(&combined).is_err());
            assert_eq!(session.feed(&bytes(&heartbeat(4))), Err(SessionStreamError::Poisoned));
        }
        let mut session = stream();
        let mut combined = bytes(&message(3, ControlMessage::Goodbye));
        combined.extend(bytes(&heartbeat(4)));
        assert_eq!(session.feed(&combined), Err(SessionStreamError::AfterGoodbye));
        assert_eq!(session.feed(&[]), Err(SessionStreamError::Poisoned));
    }

    #[test]
    fn malformed_and_truncated_frames_poison_the_entire_session() {
        let mut session = stream();
        let mut combined = bytes(&heartbeat(3));
        combined.extend([0_u8; 4]);
        assert_eq!(
            session.feed(&combined),
            Err(SessionStreamError::Decode(StreamDecodeError::MalformedFrame))
        );
        assert_eq!(session.feed(&bytes(&heartbeat(4))), Err(SessionStreamError::Poisoned));

        let mut partial = stream();
        let frame = bytes(&heartbeat(3));
        assert!(partial.feed(&frame[..5]).unwrap().is_empty());
        assert_eq!(
            partial.finish(),
            Err(SessionStreamError::Decode(StreamDecodeError::MalformedFrame))
        );
        assert_eq!(partial.feed(&frame[5..]), Err(SessionStreamError::Poisoned));
    }

    #[test]
    fn strictly_increasing_sequences_may_skip_values_but_not_repeat() {
        let mut session = stream();
        assert_eq!(session.feed(&bytes(&heartbeat(7))).unwrap(), vec![heartbeat(7)]);
        assert_eq!(session.feed(&bytes(&heartbeat(4))), Err(SessionStreamError::Replay));
    }
}

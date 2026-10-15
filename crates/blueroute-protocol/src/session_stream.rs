//! Post-hello control-stream scope and replay checks.
//!
//! This is a security boundary only for *already* verified connections. It
//! does NOT perform TLS, verify a certificate, enroll a key, or authenticate
//! a peer. Never construct it from an untrusted Hello or a raw TCP socket.
//! Production JoinNetwork must remain disabled until mutual TLS 1.3, pin
//! approval, and channel binding have been implemented by the daemon.

use std::time::Instant;

use blueroute_core::{NetworkId, NodeId};

use crate::control::{ControlEnvelope, ControlFrameError, ControlMessage, encode_control_frame};
use crate::control_io::{
    ControlIoError, DeadlineControlRead, read_one_control_frame_with_deadline,
};
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
    UnexpectedEof,
    Poisoned,
}

/// Any framing, transport or scope violation terminates this logical
/// session. A caller must discard the separately authenticated TLS channel.
#[derive(Debug)]
pub enum SessionReadIoError {
    Transport(ControlIoError),
    Scope(SessionStreamError),
    InternalFrame(ControlFrameError),
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
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<ControlEnvelope>, SessionStreamError> {
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
        // A Goodbye must be the final byte-level frame, not merely the
        // final *complete* frame. Reject even a one-byte trailing header
        // fragment immediately instead of waiting for a later EOF.
        if self.closed && self.decoder.pending_bytes() != 0 {
            return self.reject(SessionStreamError::AfterGoodbye);
        }
        Ok(frames)
    }

    /// Read exactly one bounded frame with a whole-frame absolute deadline
    /// from a caller-owned already authenticated transport. The underlying
    /// adapter must enforce blocking-read timeouts. Any read failure is
    /// terminal, including a truncated header/body or late completion.
    ///
    /// This method does not establish TLS identity, key pinning or channel
    /// binding. Those remain mandatory prerequisites for production use.
    pub fn read_one_with_deadline<R: DeadlineControlRead>(
        &mut self,
        source: &mut R,
        deadline: Instant,
    ) -> Result<ControlEnvelope, SessionReadIoError> {
        if self.poisoned {
            return Err(SessionReadIoError::Scope(SessionStreamError::Poisoned));
        }
        if self.closed {
            return Err(SessionReadIoError::Scope(SessionStreamError::AfterGoodbye));
        }
        let envelope = read_one_control_frame_with_deadline(source, deadline).map_err(|error| {
            self.poisoned = true;
            SessionReadIoError::Transport(error)
        })?;
        let bytes = encode_control_frame(&envelope).map_err(|error| {
            self.poisoned = true;
            SessionReadIoError::InternalFrame(error)
        })?;
        let mut accepted = self.feed(&bytes).map_err(SessionReadIoError::Scope)?;
        // The read primitive returns exactly one bounded validated frame.
        // Treat any unexpected disagreement as terminal, not authorization.
        if accepted.len() != 1 {
            self.poisoned = true;
            return Err(SessionReadIoError::Scope(SessionStreamError::Poisoned));
        }
        Ok(accepted.remove(0))
    }

    /// EOF is graceful only after an authenticated Goodbye and at a frame
    /// boundary. A missing Goodbye is loss, not normal close; even successful
    /// EOF irrevocably consumes the parser so it cannot be reused for a
    /// different transport or session.
    pub fn finish(&mut self) -> Result<(), SessionStreamError> {
        if self.poisoned {
            return Err(SessionStreamError::Poisoned);
        }
        match self.decoder.finish() {
            Ok(()) if self.closed => {
                self.poisoned = true;
                Ok(())
            }
            Ok(()) => self.reject(SessionStreamError::UnexpectedEof),
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
    use std::io::{self, Cursor, ErrorKind, Read};
    use std::time::Duration;

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
        assert_eq!(
            session.feed(&encoded[encoded.len() - 1..]).unwrap(),
            vec![frame]
        );
        assert_eq!(session.last_sequence(), 3);
        let goodbye = message(4, ControlMessage::Goodbye);
        assert_eq!(session.feed(&bytes(&goodbye)).unwrap(), vec![goodbye]);
        assert!(session.is_closed());
        assert_eq!(session.finish(), Ok(()));
        assert_eq!(session.finish(), Err(SessionStreamError::Poisoned));
        assert_eq!(
            session.feed(&bytes(&heartbeat(5))),
            Err(SessionStreamError::Poisoned)
        );
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
            assert_eq!(
                session.feed(&bytes(&heartbeat(3))),
                Err(SessionStreamError::Poisoned)
            );
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
            assert_eq!(
                session.feed(&bytes(&heartbeat(4))),
                Err(SessionStreamError::Poisoned)
            );
        }
        let mut session = stream();
        let mut combined = bytes(&message(3, ControlMessage::Goodbye));
        combined.extend(bytes(&heartbeat(4)));
        assert_eq!(
            session.feed(&combined),
            Err(SessionStreamError::AfterGoodbye)
        );
        assert_eq!(session.feed(&[]), Err(SessionStreamError::Poisoned));
    }

    #[test]
    fn goodbye_with_trailing_incomplete_frame_fails_atomically() {
        let goodbye = bytes(&message(3, ControlMessage::Goodbye));
        let trailing = bytes(&heartbeat(4));
        for tail_len in 1..trailing.len() {
            let mut session = stream();
            let mut combined = goodbye.clone();
            combined.extend_from_slice(&trailing[..tail_len]);
            assert_eq!(
                session.feed(&combined),
                Err(SessionStreamError::AfterGoodbye),
                "accepted trailing {tail_len}-byte fragment"
            );
            assert_eq!(
                session.feed(&bytes(&heartbeat(5))),
                Err(SessionStreamError::Poisoned)
            );
        }
    }

    #[test]
    fn malformed_and_truncated_frames_poison_the_entire_session() {
        let mut session = stream();
        let mut combined = bytes(&heartbeat(3));
        combined.extend([0_u8; 4]);
        assert_eq!(
            session.feed(&combined),
            Err(SessionStreamError::Decode(
                StreamDecodeError::MalformedFrame
            ))
        );
        assert_eq!(
            session.feed(&bytes(&heartbeat(4))),
            Err(SessionStreamError::Poisoned)
        );

        let mut partial = stream();
        let frame = bytes(&heartbeat(3));
        assert!(partial.feed(&frame[..5]).unwrap().is_empty());
        assert_eq!(
            partial.finish(),
            Err(SessionStreamError::Decode(
                StreamDecodeError::MalformedFrame
            ))
        );
        assert_eq!(partial.feed(&frame[5..]), Err(SessionStreamError::Poisoned));
    }

    #[test]
    fn abrupt_eof_before_goodbye_is_loss_and_cannot_be_reused() {
        let mut session = stream();
        assert_eq!(
            session.feed(&bytes(&heartbeat(3))).unwrap(),
            vec![heartbeat(3)]
        );
        assert_eq!(session.finish(), Err(SessionStreamError::UnexpectedEof));
        assert_eq!(
            session.feed(&bytes(&heartbeat(4))),
            Err(SessionStreamError::Poisoned)
        );

        let mut session = stream();
        assert_eq!(session.finish(), Err(SessionStreamError::UnexpectedEof));
        assert_eq!(session.finish(), Err(SessionStreamError::Poisoned));
    }

    struct DeadlineCursor {
        data: Cursor<Vec<u8>>,
        saw_deadline: bool,
    }

    impl Read for DeadlineCursor {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = bytes.len().min(1);
            self.data.read(&mut bytes[..count])
        }
    }

    impl DeadlineControlRead for DeadlineCursor {
        fn set_control_read_timeout(&mut self, _: Duration) -> io::Result<()> {
            self.saw_deadline = true;
            Ok(())
        }
    }

    fn reader(bytes: Vec<u8>) -> DeadlineCursor {
        DeadlineCursor {
            data: Cursor::new(bytes),
            saw_deadline: false,
        }
    }

    #[test]
    fn deadline_inbound_read_checks_scope_goodbye_and_refuses_reuse() {
        let mut wire = bytes(&heartbeat(3));
        wire.extend(bytes(&message(4, ControlMessage::Goodbye)));
        let mut source = reader(wire);
        let mut session = stream();
        let deadline = Instant::now() + Duration::from_secs(2);
        assert_eq!(
            session
                .read_one_with_deadline(&mut source, deadline)
                .unwrap(),
            heartbeat(3)
        );
        assert_eq!(
            session
                .read_one_with_deadline(&mut source, deadline)
                .unwrap(),
            message(4, ControlMessage::Goodbye)
        );
        assert!(source.saw_deadline);
        assert!(matches!(
            session.read_one_with_deadline(&mut source, deadline),
            Err(SessionReadIoError::Scope(SessionStreamError::AfterGoodbye))
        ));
        assert_eq!(session.finish(), Ok(()));
    }

    #[test]
    fn truncated_deadline_and_wrong_context_poison_the_reader() {
        let complete = bytes(&heartbeat(3));
        for end in [0, 1, 4, complete.len() - 1] {
            let mut session = stream();
            let mut source = reader(complete[..end].to_vec());
            let deadline = Instant::now() + Duration::from_secs(2);
            assert!(matches!(
                session.read_one_with_deadline(&mut source, deadline),
                Err(SessionReadIoError::Transport(ControlIoError::Io(ref error)))
                    if error.kind() == ErrorKind::UnexpectedEof
            ));
            assert_eq!(session.feed(&complete), Err(SessionStreamError::Poisoned));
        }

        let mut session = stream();
        let mut source = reader(complete.clone());
        assert!(matches!(
            session.read_one_with_deadline(&mut source, Instant::now()),
            Err(SessionReadIoError::Transport(ControlIoError::Io(ref error)))
                if error.kind() == ErrorKind::TimedOut
        ));
        assert_eq!(session.feed(&complete), Err(SessionStreamError::Poisoned));
        assert!(!source.saw_deadline);

        let mut wrong = heartbeat(3);
        wrong.network = NetworkId::from_bytes([9; 16]);
        let mut source = reader(bytes(&wrong));
        let mut session = stream();
        assert!(matches!(
            session.read_one_with_deadline(&mut source, Instant::now() + Duration::from_secs(2)),
            Err(SessionReadIoError::Scope(SessionStreamError::WrongContext))
        ));
        assert_eq!(session.finish(), Err(SessionStreamError::Poisoned));
    }

    #[test]
    fn strictly_increasing_sequences_may_skip_values_but_not_repeat() {
        let mut session = stream();
        assert_eq!(
            session.feed(&bytes(&heartbeat(7))).unwrap(),
            vec![heartbeat(7)]
        );
        assert_eq!(
            session.feed(&bytes(&heartbeat(4))),
            Err(SessionStreamError::Replay)
        );
    }
}

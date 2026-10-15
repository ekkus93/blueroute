//! P7-004 outbound post-hello framing on an already authenticated session.
//!
//! This module does not authenticate, conduct TLS, verify a key pin or enroll
//! any identity. Only a separately verified TLS 1.3 connection with a pinned
//! peer and exporter-bound Hello transcript may construct the writer. Never
//! attach it to raw TCP. P6-004 production JoinNetwork remains fail-closed.

use std::time::Instant;

use blueroute_core::{NetworkId, NodeId};

use crate::control::{
    CONTROL_VERSION, ControlEnvelope, ControlFrameError, ControlMessage, decode_control_frame,
    encode_control_frame,
};
use crate::control_io::{ControlIoError, DeadlineControlWrite, write_one_control_frame_with_deadline};

/// Monotonic local sequence allocation after Hello(sequence=1) and
/// HelloAck(sequence=2). This writer does not handle handshake messages,
/// remote claims or membership/route-changing operations.
#[derive(Debug)]
pub struct SessionControlWriter {
    network: NetworkId,
    local: NodeId,
    exporter_session_id: [u8; 16],
    next_sequence: u64,
    closed: bool,
    poisoned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionWriteError {
    InvalidSession,
    AfterGoodbye,
    SequenceExhausted,
    Encode(ControlFrameError),
    Poisoned,
}

/// Errors from the bounded transport path. A partial write always poisons
/// the writer; the TLS channel must be discarded, not retried or resumed.
#[derive(Debug)]
pub enum SessionWriteIoError {
    State(SessionWriteError),
    Transport(ControlIoError),
    InternalFrame(ControlFrameError),
}

impl SessionControlWriter {
    /// Only call after independent mutual-TLS verification, user-approved
    /// (NetworkId, peer NodeId, SPKI-SHA256) pin checks and a symmetric
    /// verified Hello/HelloAck bound to the TLS exporter from that channel.
    /// These prerequisites are documented, NOT established by this method.
    pub fn after_verified_hello(
        network: NetworkId,
        local: NodeId,
        peer: NodeId,
        exporter_session_id: [u8; 16],
    ) -> Result<Self, SessionWriteError> {
        if local == peer || exporter_session_id == [0; 16] {
            return Err(SessionWriteError::InvalidSession);
        }
        Ok(Self {
            network,
            local,
            exporter_session_id,
            next_sequence: 3,
            closed: false,
            poisoned: false,
        })
    }

    pub fn heartbeat(&mut self) -> Result<Vec<u8>, SessionWriteError> {
        self.encode(ControlMessage::Heartbeat)
    }

    /// Terminal: no further outbound frames may be encoded. If a transport
    /// write fails partway, drop the entire TLS session, never retry the same
    /// frame on a new socket or declare a graceful Goodbye.
    pub fn goodbye(&mut self) -> Result<Vec<u8>, SessionWriteError> {
        self.encode(ControlMessage::Goodbye)
    }

    /// Write one scoped heartbeat with a whole-frame absolute deadline.
    /// An I/O error (including late completion) irreversibly poisons the
    /// session; callers MUST close the separately authenticated TLS channel.
    pub fn write_heartbeat_with_deadline<W: DeadlineControlWrite>(
        &mut self,
        destination: &mut W,
        deadline: Instant,
    ) -> Result<(), SessionWriteIoError> {
        self.write_message_with_deadline(destination, deadline, ControlMessage::Heartbeat)
    }

    /// Write terminal Goodbye; successful issuance closes this writer.
    /// The caller must still close its authenticated transport gracefully.
    pub fn write_goodbye_with_deadline<W: DeadlineControlWrite>(
        &mut self,
        destination: &mut W,
        deadline: Instant,
    ) -> Result<(), SessionWriteIoError> {
        self.write_message_with_deadline(destination, deadline, ControlMessage::Goodbye)
    }

    fn write_message_with_deadline<W: DeadlineControlWrite>(
        &mut self,
        destination: &mut W,
        deadline: Instant,
        message: ControlMessage,
    ) -> Result<(), SessionWriteIoError> {
        let frame = self.encode(message).map_err(SessionWriteIoError::State)?;
        // The writer creates only validated canonical frames. Recheck before
        // calling the existing absolute-deadline I/O primitive, whose API
        // accepts a typed envelope rather than arbitrary serialized bytes.
        let envelope = decode_control_frame(&frame).map_err(|error| {
            self.poisoned = true;
            SessionWriteIoError::InternalFrame(error)
        })?;
        if let Err(error) = write_one_control_frame_with_deadline(destination, &envelope, deadline) {
            self.poisoned = true;
            return Err(SessionWriteIoError::Transport(error));
        }
        Ok(())
    }

    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    pub const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    fn encode(&mut self, message: ControlMessage) -> Result<Vec<u8>, SessionWriteError> {
        if self.poisoned {
            return Err(SessionWriteError::Poisoned);
        }
        if self.closed {
            return Err(SessionWriteError::AfterGoodbye);
        }
        // Refuse a counter rollover rather than reusing an earlier value.
        let Some(next) = self.next_sequence.checked_add(1) else {
            self.poisoned = true;
            return Err(SessionWriteError::SequenceExhausted);
        };
        let terminal = matches!(message, ControlMessage::Goodbye);
        let envelope = ControlEnvelope {
            version: CONTROL_VERSION,
            network: self.network,
            sender: self.local,
            session_id: self.exporter_session_id,
            sequence: self.next_sequence,
            message,
        };
        let encoded = encode_control_frame(&envelope).map_err(|error| {
            self.poisoned = true;
            SessionWriteError::Encode(error)
        })?;
        self.next_sequence = next;
        if terminal {
            self.closed = true;
        }
        Ok(encoded)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, ErrorKind, Write};
    use std::time::Duration;

    use super::*;
    use crate::session_stream::{SessionControlStream, SessionStreamError};

    fn network() -> NetworkId {
        NetworkId::from_bytes([1; 16])
    }

    fn alice() -> NodeId {
        NodeId::from_bytes([2; 16])
    }

    fn bob() -> NodeId {
        NodeId::from_bytes([3; 16])
    }

    fn outbound() -> SessionControlWriter {
        SessionControlWriter::after_verified_hello(network(), alice(), bob(), [4; 16]).unwrap()
    }

    #[test]
    fn writer_and_inbound_stream_agree_on_network_sender_exporter_and_sequence() {
        let mut writer = outbound();
        let mut inbound =
            SessionControlStream::after_verified_hello(network(), alice(), [4; 16]).unwrap();
        let mut wire = Vec::new();
        wire.extend(writer.heartbeat().unwrap());
        wire.extend(writer.heartbeat().unwrap());
        wire.extend(writer.goodbye().unwrap());
        assert!(writer.is_closed());
        assert_eq!(writer.next_sequence(), 6);

        let mut decoded = Vec::new();
        for byte in wire {
            decoded.extend(inbound.feed(&[byte]).unwrap());
        }
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].sequence, 3);
        assert_eq!(decoded[1].sequence, 4);
        assert_eq!(decoded[2].sequence, 5);
        assert_eq!(decoded[0].sender, alice());
        assert!(matches!(decoded[2].message, ControlMessage::Goodbye));
        assert_eq!(inbound.finish(), Ok(()));
        assert_eq!(writer.heartbeat(), Err(SessionWriteError::AfterGoodbye));
        assert_eq!(writer.goodbye(), Err(SessionWriteError::AfterGoodbye));
    }

    #[test]
    fn invalid_context_is_rejected_and_sessions_do_not_cross() {
        assert!(matches!(
            SessionControlWriter::after_verified_hello(network(), alice(), alice(), [4; 16]),
            Err(SessionWriteError::InvalidSession)
        ));
        assert!(matches!(
            SessionControlWriter::after_verified_hello(network(), alice(), bob(), [0; 16]),
            Err(SessionWriteError::InvalidSession)
        ));

        let mut writer = outbound();
        let frame = writer.heartbeat().unwrap();
        let decoded = decode_control_frame(&frame).unwrap();
        assert_eq!(decoded.network, network());
        assert_eq!(decoded.session_id, [4; 16]);
        let mut wrong_peer =
            SessionControlStream::after_verified_hello(network(), bob(), [4; 16]).unwrap();
        assert_eq!(
            wrong_peer.feed(&frame),
            Err(SessionStreamError::WrongContext)
        );
        let mut wrong_exporter =
            SessionControlStream::after_verified_hello(network(), alice(), [5; 16]).unwrap();
        assert_eq!(
            wrong_exporter.feed(&frame),
            Err(SessionStreamError::WrongContext)
        );
    }

    struct DeadlineBuffer {
        bytes: Vec<u8>,
        fail_after: Option<usize>,
        seen_timeout: bool,
    }

    impl Write for DeadlineBuffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_after.is_some_and(|limit| self.bytes.len() >= limit) {
                return Err(io::Error::from(ErrorKind::BrokenPipe));
            }
            let count = bytes.len().min(1);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl DeadlineControlWrite for DeadlineBuffer {
        fn set_control_write_timeout(&mut self, _: Duration) -> io::Result<()> {
            self.seen_timeout = true;
            Ok(())
        }
    }

    #[test]
    fn deadline_writer_emits_strict_frames_and_closes_after_goodbye() {
        let mut writer = outbound();
        let mut destination = DeadlineBuffer {
            bytes: Vec::new(),
            fail_after: None,
            seen_timeout: false,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        writer
            .write_heartbeat_with_deadline(&mut destination, deadline)
            .unwrap();
        let first = decode_control_frame(&destination.bytes).unwrap();
        assert_eq!(first.sequence, 3);
        assert!(destination.seen_timeout);
        destination.bytes.clear();
        writer
            .write_goodbye_with_deadline(&mut destination, deadline)
            .unwrap();
        let final_frame = decode_control_frame(&destination.bytes).unwrap();
        assert_eq!(final_frame.sequence, 4);
        assert!(matches!(final_frame.message, ControlMessage::Goodbye));
        assert!(matches!(
            writer.write_heartbeat_with_deadline(&mut destination, deadline),
            Err(SessionWriteIoError::State(SessionWriteError::AfterGoodbye))
        ));
    }

    #[test]
    fn partial_write_and_expired_deadline_poison_the_session() {
        let mut writer = outbound();
        let mut destination = DeadlineBuffer {
            bytes: Vec::new(),
            fail_after: Some(7),
            seen_timeout: false,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        assert!(matches!(
            writer.write_heartbeat_with_deadline(&mut destination, deadline),
            Err(SessionWriteIoError::Transport(ControlIoError::Io(ref error)))
                if error.kind() == ErrorKind::BrokenPipe
        ));
        assert_eq!(destination.bytes.len(), 7);
        assert_eq!(writer.heartbeat(), Err(SessionWriteError::Poisoned));

        let mut expired = outbound();
        let mut intact = DeadlineBuffer {
            bytes: Vec::new(),
            fail_after: None,
            seen_timeout: false,
        };
        assert!(matches!(
            expired.write_heartbeat_with_deadline(&mut intact, Instant::now()),
            Err(SessionWriteIoError::Transport(ControlIoError::Io(ref error)))
                if error.kind() == ErrorKind::TimedOut
        ));
        assert!(intact.bytes.is_empty());
        assert_eq!(expired.goodbye(), Err(SessionWriteError::Poisoned));
    }

    #[test]
    fn sequence_exhaustion_and_goodbye_are_irreversible() {
        let mut writer = outbound();
        writer.next_sequence = u64::MAX - 1;
        let frame = writer.heartbeat().unwrap();
        assert_eq!(decode_control_frame(&frame).unwrap().sequence, u64::MAX - 1);
        assert_eq!(writer.next_sequence(), u64::MAX);
        assert_eq!(writer.goodbye(), Err(SessionWriteError::SequenceExhausted));
        assert_eq!(writer.heartbeat(), Err(SessionWriteError::Poisoned));
        assert!(!writer.is_closed());

        let mut closed = outbound();
        let bye = closed.goodbye().unwrap();
        assert!(matches!(
            decode_control_frame(&bye).unwrap().message,
            ControlMessage::Goodbye
        ));
        assert_eq!(closed.heartbeat(), Err(SessionWriteError::AfterGoodbye));
    }
}

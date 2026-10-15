//! Deterministic idle/EOF lifecycle for a *separately authenticated* P7 control stream.
//!
//! No cryptographic verification is provided here. A daemon may only construct
//! this state after mutual TLS 1.3, enrolled peer-key proof, and an exporter-bound
//! Hello/HelloAck. Raw TCP frames and untrusted claims must never authorize join.

use blueroute_core::{NetworkId, NodeId};

use crate::control::{ControlEnvelope, ControlMessage};
use crate::session::{ConnectionDeadline, ConnectionStage, DeadlineError};
use crate::session_stream::{SessionControlStream, SessionStreamError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InboundActivityError {
    NotActive,
    Deadline(DeadlineError),
    Scope(SessionStreamError),
    Terminal,
}

/// Couples the already-verified session's parser with its monotonic idle clock.
/// A partial frame, invalid frame or empty poll cannot refresh liveness.
/// Goodbye transitions to bounded closing and EOF irrevocably consumes state.
/// Every error is terminal: no socket replacement or auth retry in-place.
#[derive(Debug)]
pub struct AuthenticatedInboundActivity {
    stream: SessionControlStream,
    deadline: ConnectionDeadline,
    terminal: bool,
    goodbye_seen: bool,
}

impl AuthenticatedInboundActivity {
    /// `deadline` must have already passed the connection/TLS/Hello stages
    /// through authenticated validation. This constructor does not do so.
    pub fn after_verified_hello(
        network: NetworkId,
        peer: NodeId,
        exported_session_id: [u8; 16],
        deadline: ConnectionDeadline,
    ) -> Result<Self, InboundActivityError> {
        if deadline.stage() != ConnectionStage::Active {
            return Err(InboundActivityError::NotActive);
        }
        let stream = SessionControlStream::after_verified_hello(network, peer, exported_session_id)
            .map_err(InboundActivityError::Scope)?;
        Ok(Self {
            stream,
            deadline,
            terminal: false,
            goodbye_seen: false,
        })
    }

    /// Accept plaintext only *after* external mutual-TLS processing. The
    /// monotonic instant is supplied by the trusted local clock, not a peer.
    /// Scope/replay validation always precedes idle-deadline renewal.
    pub fn feed_verified_plaintext(
        &mut self,
        chunk: &[u8],
        now_ms: u64,
    ) -> Result<Vec<ControlEnvelope>, InboundActivityError> {
        if self.terminal {
            return Err(InboundActivityError::Terminal);
        }
        if self.goodbye_seen {
            self.terminal = true;
            return Err(InboundActivityError::Terminal);
        }
        if let Err(error) = self.deadline.check(now_ms) {
            self.terminal = true;
            return Err(InboundActivityError::Deadline(error));
        }
        let frames = match self.stream.feed(chunk) {
            Ok(frames) => frames,
            Err(error) => {
                self.terminal = true;
                return Err(InboundActivityError::Scope(error));
            }
        };
        if frames.is_empty() {
            return Ok(frames);
        }
        if frames
            .iter()
            .any(|frame| matches!(frame.message, ControlMessage::Goodbye))
        {
            if let Err(error) = self.deadline.abort(now_ms) {
                self.terminal = true;
                return Err(InboundActivityError::Deadline(error));
            }
            self.goodbye_seen = true;
        } else if let Err(error) = self.deadline.authenticated_activity(now_ms) {
            self.terminal = true;
            return Err(InboundActivityError::Deadline(error));
        }
        Ok(frames)
    }

    /// Only a valid terminal Goodbye followed by clean EOF is graceful.
    /// EOF, graceful or not, permanently consumes the logical session.
    /// Enforce the closing deadline even if the peer sent a valid Goodbye:
    /// it must not hold a daemon socket or task open indefinitely.
    pub fn finish_at(&mut self, now_ms: u64) -> Result<(), InboundActivityError> {
        if self.terminal {
            return Err(InboundActivityError::Terminal);
        }
        if let Err(error) = self.deadline.check(now_ms) {
            self.terminal = true;
            return Err(InboundActivityError::Deadline(error));
        }
        self.terminal = true;
        self.stream.finish().map_err(InboundActivityError::Scope)
    }

    pub const fn expires_ms(&self) -> u64 {
        self.deadline.expires_ms()
    }

    pub const fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub const fn goodbye_seen(&self) -> bool {
        self.goodbye_seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{CONTROL_VERSION, ControlEnvelope, encode_control_frame};

    fn network() -> NetworkId {
        NetworkId::from_bytes([1; 16])
    }

    fn peer() -> NodeId {
        NodeId::from_bytes([2; 16])
    }

    fn active_deadline() -> ConnectionDeadline {
        let mut deadline = ConnectionDeadline::new(0);
        deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
        deadline.advance(ConnectionStage::Hello, 2).unwrap();
        deadline.advance(ConnectionStage::Active, 3).unwrap();
        deadline
    }

    fn activity() -> AuthenticatedInboundActivity {
        AuthenticatedInboundActivity::after_verified_hello(
            network(),
            peer(),
            [3; 16],
            active_deadline(),
        )
        .unwrap()
    }

    fn wire(sender: NodeId, sequence: u64, message: ControlMessage) -> Vec<u8> {
        encode_control_frame(&ControlEnvelope {
            version: CONTROL_VERSION,
            network: network(),
            sender,
            session_id: [3; 16],
            sequence,
            message,
        })
        .unwrap()
    }

    #[test]
    fn only_complete_checked_frames_refresh_the_idle_deadline() {
        let mut session = activity();
        let heartbeat = wire(peer(), 3, ControlMessage::Heartbeat);
        let initial_expiry = session.expires_ms();
        assert_eq!(initial_expiry, 30_003);
        assert!(
            session
                .feed_verified_plaintext(&heartbeat[..4], 29_900)
                .unwrap()
                .is_empty()
        );
        assert_eq!(session.expires_ms(), initial_expiry);
        assert_eq!(
            session
                .feed_verified_plaintext(&heartbeat[4..], 29_999)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(session.expires_ms(), 59_999);
        assert!(
            session
                .feed_verified_plaintext(&[], 59_998)
                .unwrap()
                .is_empty()
        );
        assert_eq!(session.expires_ms(), 59_999);
        assert_eq!(
            session.feed_verified_plaintext(&[], 59_999),
            Err(InboundActivityError::Deadline(DeadlineError::Expired))
        );
        assert!(session.is_terminal());
        assert_eq!(
            session.feed_verified_plaintext(&heartbeat, 60_000),
            Err(InboundActivityError::Terminal)
        );
    }

    #[test]
    fn partial_frame_across_timeout_never_becomes_authorized_activity() {
        let mut session = activity();
        let frame = wire(peer(), 3, ControlMessage::Heartbeat);
        assert!(
            session
                .feed_verified_plaintext(&frame[..frame.len() - 1], 29_999)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            session.feed_verified_plaintext(&frame[frame.len() - 1..], 30_003),
            Err(InboundActivityError::Deadline(DeadlineError::Expired))
        );
        assert!(session.is_terminal());
    }

    #[test]
    fn untrusted_peer_or_clock_regression_is_irreversibly_terminal() {
        let mut session = activity();
        let wrong = wire(NodeId::from_bytes([9; 16]), 3, ControlMessage::Heartbeat);
        assert_eq!(
            session.feed_verified_plaintext(&wrong, 10),
            Err(InboundActivityError::Scope(
                SessionStreamError::WrongContext
            ))
        );
        assert_eq!(
            session.feed_verified_plaintext(&wire(peer(), 3, ControlMessage::Heartbeat), 11),
            Err(InboundActivityError::Terminal)
        );

        let mut another = activity();
        assert!(another.feed_verified_plaintext(&[], 15).unwrap().is_empty());
        // The deadline's last observed time only advances on authenticated
        // frames. A valid heartbeat at t=20 makes subsequent t=19 invalid.
        assert_eq!(
            another
                .feed_verified_plaintext(&wire(peer(), 3, ControlMessage::Heartbeat), 20)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            another.feed_verified_plaintext(&[], 19),
            Err(InboundActivityError::Deadline(
                DeadlineError::ClockRegression
            ))
        );
    }

    #[test]
    fn goodbye_requires_clean_eof_and_never_resurrects() {
        let mut session = activity();
        assert_eq!(
            session
                .feed_verified_plaintext(&wire(peer(), 3, ControlMessage::Goodbye), 10)
                .unwrap()
                .len(),
            1
        );
        assert!(session.goodbye_seen());
        assert_eq!(session.expires_ms(), 5_010);
        assert_eq!(session.finish_at(11), Ok(()));
        assert!(session.is_terminal());
        assert_eq!(
            session.feed_verified_plaintext(&wire(peer(), 4, ControlMessage::Heartbeat), 12),
            Err(InboundActivityError::Terminal)
        );
        assert_eq!(session.finish_at(12), Err(InboundActivityError::Terminal));

        let mut abrupt = activity();
        assert_eq!(
            abrupt.finish_at(11),
            Err(InboundActivityError::Scope(
                SessionStreamError::UnexpectedEof
            ))
        );
        assert!(abrupt.is_terminal());
    }

    #[test]
    fn goodbye_eof_is_bounded_and_post_goodbye_input_is_terminal() {
        let mut timed_out = activity();
        timed_out
            .feed_verified_plaintext(&wire(peer(), 3, ControlMessage::Goodbye), 10)
            .unwrap();
        assert_eq!(
            timed_out.finish_at(5_010),
            Err(InboundActivityError::Deadline(DeadlineError::Expired))
        );
        assert!(timed_out.is_terminal());
        assert_eq!(
            timed_out.finish_at(5_009),
            Err(InboundActivityError::Terminal)
        );

        let mut post_goodbye = activity();
        post_goodbye
            .feed_verified_plaintext(&wire(peer(), 3, ControlMessage::Goodbye), 10)
            .unwrap();
        assert_eq!(
            post_goodbye.feed_verified_plaintext(&[1], 11),
            Err(InboundActivityError::Terminal)
        );
        assert!(post_goodbye.is_terminal());
        assert_eq!(
            post_goodbye.finish_at(12),
            Err(InboundActivityError::Terminal)
        );
    }

    #[test]
    fn rejects_unverified_deadline_stages_and_zero_exporter() {
        assert!(matches!(
            AuthenticatedInboundActivity::after_verified_hello(
                network(),
                peer(),
                [3; 16],
                ConnectionDeadline::new(0)
            ),
            Err(InboundActivityError::NotActive)
        ));
        assert!(matches!(
            AuthenticatedInboundActivity::after_verified_hello(
                network(),
                peer(),
                [0; 16],
                active_deadline()
            ),
            Err(InboundActivityError::Scope(
                SessionStreamError::InvalidSession
            ))
        ));
    }
}

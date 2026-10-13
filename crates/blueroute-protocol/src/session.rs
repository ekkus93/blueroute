//! Deterministic logical control-session lifecycle and stale-event handling.
//!
//! This module performs no cryptography. Only the future authenticated TLS
//! session layer may call activate_after_authentication; no daemon path uses
//! this registry yet. Never construct an authorized member from this table.

use std::collections::BTreeMap;
use std::fmt;

use blueroute_core::{NetworkId, NodeId};

pub const MAX_CONTROL_SESSIONS: usize = 128;

/// Software-only retry policy for a future authenticated control transport.
/// The transport must also impose connect/handshake timeouts and cancellation.
/// Retrying never grants authorization and must not bypass the TLS trust gate.
pub const MAX_RECONNECT_ATTEMPTS: u8 = 8;
pub const INITIAL_RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
pub const MAX_RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

/// Deterministic exponential backoff. Each lost session gets its own state,
/// preventing an unbounded reconnect spin loop after failures.
/// Monotonic-clock deadline policy; not a TLS verifier. A real transport
/// must enforce these budgets with cancellable network I/O.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionStage {
    Connecting,
    TlsHandshake,
    Hello,
    Active,
    Closing,
}

impl ConnectionStage {
    pub const fn timeout_ms(self) -> u64 {
        match self {
            Self::Connecting | Self::Hello | Self::Closing => 5_000,
            Self::TlsHandshake => 10_000,
            Self::Active => 30_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineError {
    Expired,
    ClockRegression,
    InvalidTransition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionDeadline {
    stage: ConnectionStage,
    last_ms: u64,
    expires_ms: u64,
}

impl ConnectionDeadline {
    pub fn new(now_ms: u64) -> Self {
        Self {
            stage: ConnectionStage::Connecting,
            last_ms: now_ms,
            expires_ms: now_ms.saturating_add(ConnectionStage::Connecting.timeout_ms()),
        }
    }

    pub const fn stage(&self) -> ConnectionStage {
        self.stage
    }

    pub const fn expires_ms(&self) -> u64 {
        self.expires_ms
    }

    pub fn check(&self, now_ms: u64) -> Result<(), DeadlineError> {
        if now_ms < self.last_ms {
            return Err(DeadlineError::ClockRegression);
        }
        if now_ms >= self.expires_ms {
            return Err(DeadlineError::Expired);
        }
        Ok(())
    }

    pub fn advance(&mut self, next: ConnectionStage, now_ms: u64) -> Result<(), DeadlineError> {
        self.check(now_ms)?;
        let allowed = matches!(
            (self.stage, next),
            (ConnectionStage::Connecting, ConnectionStage::TlsHandshake)
                | (ConnectionStage::TlsHandshake, ConnectionStage::Hello)
                | (ConnectionStage::Hello, ConnectionStage::Active)
                | (ConnectionStage::Active, ConnectionStage::Closing)
        );
        if !allowed {
            return Err(DeadlineError::InvalidTransition);
        }
        self.stage = next;
        self.last_ms = now_ms;
        self.expires_ms = now_ms.saturating_add(next.timeout_ms());
        Ok(())
    }

    /// Begin bounded cleanup even when an earlier deadline expired.
    /// A repeated abort never extends an existing cleanup deadline.
    pub fn abort(&mut self, now_ms: u64) -> Result<(), DeadlineError> {
        if now_ms < self.last_ms {
            return Err(DeadlineError::ClockRegression);
        }
        if self.stage == ConnectionStage::Closing {
            return Ok(());
        }
        self.stage = ConnectionStage::Closing;
        self.last_ms = now_ms;
        self.expires_ms = now_ms.saturating_add(ConnectionStage::Closing.timeout_ms());
        Ok(())
    }

    /// Call only after a complete, authenticated and replay-checked message.
    /// Raw bytes and unverified heartbeat claims cannot refresh idle time.
    pub fn authenticated_activity(&mut self, now_ms: u64) -> Result<(), DeadlineError> {
        self.check(now_ms)?;
        if self.stage != ConnectionStage::Active {
            return Err(DeadlineError::InvalidTransition);
        }
        self.last_ms = now_ms;
        self.expires_ms = now_ms.saturating_add(ConnectionStage::Active.timeout_ms());
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReconnectBackoff {
    failed_attempts: u8,
}

impl ReconnectBackoff {
    pub const fn new() -> Self {
        Self { failed_attempts: 0 }
    }

    pub const fn failed_attempts(&self) -> u8 {
        self.failed_attempts
    }

    /// Return the delay before another attempt, or None when the budget is
    /// exhausted. Exponents are bounded by MAX_RECONNECT_ATTEMPTS.
    pub fn after_failure(&mut self) -> Option<std::time::Duration> {
        if self.failed_attempts >= MAX_RECONNECT_ATTEMPTS {
            return None;
        }
        let multiplier = 1_u64 << u32::from(self.failed_attempts);
        let milliseconds = (250_u64 * multiplier).min(30_000);
        self.failed_attempts += 1;
        Some(std::time::Duration::from_millis(milliseconds))
    }

    /// Reset only after an authenticated session is established successfully
    /// (or when the caller explicitly cancels its entire retry lifecycle).
    pub fn reset(&mut self) {
        self.failed_attempts = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PeerSessionKey {
    pub network: NetworkId,
    pub peer: NodeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPhase {
    Active,
    Lost,
    Reconnecting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionEntry {
    pub session_id: [u8; 16],
    pub phase: SessionPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionRegistration {
    New,
    AlreadyActive,
    Recovered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionRegistryError {
    ZeroSessionId,
    DuplicateActiveSession,
    SessionLimitReached,
}

impl fmt::Display for SessionRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSessionId => f.write_str("invalid zero session id"),
            Self::DuplicateActiveSession => {
                f.write_str("a different session is already active for this peer")
            }
            Self::SessionLimitReached => f.write_str("control session capacity reached"),
        }
    }
}

impl std::error::Error for SessionRegistryError {}

#[derive(Debug, Default)]
pub struct ControlSessionRegistry {
    sessions: BTreeMap<PeerSessionKey, SessionEntry>,
}

impl ControlSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn get(&self, key: PeerSessionKey) -> Option<SessionEntry> {
        self.sessions.get(&key).copied()
    }

    /// Call ONLY after mutual TLS, pinned-key NodeId and NetworkId approval
    /// and channel-bound fresh nonce are verified by the future P7-004 layer.
    ///
    /// Rejects concurrent active sessions for the same logical peer. Once
    /// a session is marked lost, replacement by a fresh verified one is safe.
    pub fn activate_after_authentication(
        &mut self,
        key: PeerSessionKey,
        session_id: [u8; 16],
    ) -> Result<SessionRegistration, SessionRegistryError> {
        if session_id.iter().all(|byte| *byte == 0) {
            return Err(SessionRegistryError::ZeroSessionId);
        }
        if self
            .sessions
            .iter()
            .any(|(other, existing)| *other != key && existing.session_id == session_id)
        {
            return Err(SessionRegistryError::DuplicateActiveSession);
        }
        if let Some(existing) = self.sessions.get_mut(&key) {
            if existing.phase != SessionPhase::Active && existing.session_id == session_id {
                return Err(SessionRegistryError::DuplicateActiveSession);
            }
            if existing.phase == SessionPhase::Active {
                return if existing.session_id == session_id {
                    Ok(SessionRegistration::AlreadyActive)
                } else {
                    Err(SessionRegistryError::DuplicateActiveSession)
                };
            }
            *existing = SessionEntry {
                session_id,
                phase: SessionPhase::Active,
            };
            return Ok(SessionRegistration::Recovered);
        }
        if self.sessions.len() >= MAX_CONTROL_SESSIONS {
            return Err(SessionRegistryError::SessionLimitReached);
        }
        self.sessions.insert(
            key,
            SessionEntry {
                session_id,
                phase: SessionPhase::Active,
            },
        );
        Ok(SessionRegistration::New)
    }

    /// Returns false for a stale loss from a previously superseded connection.
    pub fn mark_lost(&mut self, key: PeerSessionKey, session_id: [u8; 16]) -> bool {
        let Some(existing) = self.sessions.get_mut(&key) else {
            return false;
        };
        if existing.session_id != session_id {
            return false;
        }
        existing.phase = SessionPhase::Lost;
        true
    }

    /// Called by a bounded retry controller (not implemented here).
    pub fn mark_reconnecting(&mut self, key: PeerSessionKey, session_id: [u8; 16]) -> bool {
        let Some(existing) = self.sessions.get_mut(&key) else {
            return false;
        };
        if existing.session_id != session_id || existing.phase == SessionPhase::Active {
            return false;
        }
        existing.phase = SessionPhase::Reconnecting;
        true
    }

    /// Session-scoped, idempotent cleanup. An old task cannot drop a new peer.
    pub fn remove_if_current(&mut self, key: PeerSessionKey, session_id: [u8; 16]) -> bool {
        if self
            .sessions
            .get(&key)
            .is_some_and(|existing| existing.session_id == session_id)
        {
            self.sessions.remove(&key);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: u8) -> PeerSessionKey {
        PeerSessionKey {
            network: NetworkId::from_bytes([1; 16]),
            peer: NodeId::from_bytes([value; 16]),
        }
    }

    #[test]
    fn active_duplicates_fail_closed_but_exact_retry_is_idempotent() {
        let mut sessions = ControlSessionRegistry::new();
        assert_eq!(
            sessions.activate_after_authentication(key(2), [3; 16]),
            Ok(SessionRegistration::New)
        );
        assert_eq!(
            sessions.activate_after_authentication(key(2), [3; 16]),
            Ok(SessionRegistration::AlreadyActive)
        );
        assert_eq!(
            sessions.activate_after_authentication(key(2), [4; 16]),
            Err(SessionRegistryError::DuplicateActiveSession)
        );
        assert_eq!(
            sessions.activate_after_authentication(key(2), [0; 16]),
            Err(SessionRegistryError::ZeroSessionId)
        );
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn stale_disconnect_never_closes_new_recovered_session() {
        let mut sessions = ControlSessionRegistry::new();
        sessions
            .activate_after_authentication(key(2), [3; 16])
            .unwrap();
        assert!(sessions.mark_lost(key(2), [3; 16]));
        assert!(sessions.mark_reconnecting(key(2), [3; 16]));
        assert_eq!(
            sessions.activate_after_authentication(key(2), [4; 16]),
            Ok(SessionRegistration::Recovered)
        );
        assert!(!sessions.mark_lost(key(2), [3; 16]));
        assert!(!sessions.remove_if_current(key(2), [3; 16]));
        assert_eq!(
            sessions.get(key(2)),
            Some(SessionEntry {
                session_id: [4; 16],
                phase: SessionPhase::Active
            })
        );
        assert!(!sessions.mark_reconnecting(key(2), [4; 16]));
        assert!(sessions.remove_if_current(key(2), [4; 16]));
        assert!(!sessions.remove_if_current(key(2), [4; 16]));
        assert!(sessions.is_empty());
    }

    #[test]
    fn session_ids_are_not_reused_across_peers_or_reconnections() {
        let mut registry = ControlSessionRegistry::new();
        registry
            .activate_after_authentication(key(1), [7; 16])
            .unwrap();
        assert_eq!(
            registry.activate_after_authentication(key(2), [7; 16]),
            Err(SessionRegistryError::DuplicateActiveSession)
        );
        assert!(registry.mark_lost(key(1), [7; 16]));
        assert_eq!(
            registry.activate_after_authentication(key(1), [7; 16]),
            Err(SessionRegistryError::DuplicateActiveSession)
        );
        assert_eq!(
            registry.activate_after_authentication(key(1), [8; 16]),
            Ok(SessionRegistration::Recovered)
        );
    }

    #[test]
    fn deadline_enforces_verified_stage_order_and_idle_expiry() {
        let mut clock = ConnectionDeadline::new(100);
        assert_eq!(
            clock.advance(ConnectionStage::Active, 101),
            Err(DeadlineError::InvalidTransition)
        );
        clock.advance(ConnectionStage::TlsHandshake, 200).unwrap();
        clock.advance(ConnectionStage::Hello, 300).unwrap();
        assert_eq!(
            clock.authenticated_activity(400),
            Err(DeadlineError::InvalidTransition)
        );
        clock.advance(ConnectionStage::Active, 500).unwrap();
        assert_eq!(clock.expires_ms(), 30_500);
        clock.authenticated_activity(30_499).unwrap();
        assert_eq!(clock.check(60_498), Ok(()));
        assert_eq!(clock.check(60_499), Err(DeadlineError::Expired));
    }

    #[test]
    fn deadline_rejects_clock_regression_and_exact_timeout() {
        let mut clock = ConnectionDeadline::new(1_000);
        assert_eq!(
            clock.advance(ConnectionStage::TlsHandshake, 6_000),
            Err(DeadlineError::Expired)
        );
        clock.advance(ConnectionStage::TlsHandshake, 5_999).unwrap();
        assert_eq!(
            clock.advance(ConnectionStage::Hello, 5_998),
            Err(DeadlineError::ClockRegression)
        );
        assert_eq!(clock.stage(), ConnectionStage::TlsHandshake);
        let clock = ConnectionDeadline::new(u64::MAX - 1);
        assert_eq!(clock.check(u64::MAX), Err(DeadlineError::Expired));
    }

    #[test]
    fn timed_out_handshake_can_abort_without_reopening_or_extending_cleanup() {
        let mut deadline = ConnectionDeadline::new(100);
        assert_eq!(deadline.check(5_100), Err(DeadlineError::Expired));
        deadline.abort(5_100).unwrap();
        assert_eq!(deadline.stage(), ConnectionStage::Closing);
        assert_eq!(deadline.expires_ms(), 10_100);
        deadline.abort(6_000).unwrap();
        assert_eq!(deadline.expires_ms(), 10_100);
        assert_eq!(
            deadline.advance(ConnectionStage::Active, 6_001),
            Err(DeadlineError::InvalidTransition)
        );
        assert_eq!(
            deadline.abort(5_000),
            Err(DeadlineError::ClockRegression)
        );
    }

    #[test]
    fn reconnect_backoff_is_bounded_and_requires_explicit_reset() {
        let mut retry = ReconnectBackoff::new();
        let expected_ms = [250, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
        for (number, millis) in expected_ms.iter().enumerate() {
            assert_eq!(
                retry.after_failure(),
                Some(std::time::Duration::from_millis(*millis))
            );
            assert_eq!(usize::from(retry.failed_attempts()), number + 1);
        }
        for _ in 0..16 {
            assert_eq!(retry.after_failure(), None);
        }
        assert_eq!(retry.failed_attempts(), MAX_RECONNECT_ATTEMPTS);
        retry.reset();
        assert_eq!(retry.after_failure(), Some(INITIAL_RECONNECT_DELAY));
    }

    #[test]
    fn reconnect_backoff_state_is_independent_between_peers() {
        let mut first = ReconnectBackoff::new();
        let mut second = ReconnectBackoff::new();
        assert_eq!(first.after_failure(), Some(INITIAL_RECONNECT_DELAY));
        assert_eq!(
            first.after_failure(),
            Some(std::time::Duration::from_millis(500))
        );
        assert_eq!(second.after_failure(), Some(INITIAL_RECONNECT_DELAY));
        assert_eq!(first.failed_attempts(), 2);
        assert_eq!(second.failed_attempts(), 1);
        assert_eq!(MAX_RECONNECT_DELAY, std::time::Duration::from_secs(30));
    }

    #[test]
    fn simultaneous_distinct_peers_and_capacity_are_bounded() {
        let mut sessions = ControlSessionRegistry::new();
        for i in 0..MAX_CONTROL_SESSIONS {
            let peer = PeerSessionKey {
                network: NetworkId::from_bytes([(i >> 8) as u8; 16]),
                peer: NodeId::from_bytes([(i + 1) as u8; 16]),
            };
            assert_eq!(
                sessions.activate_after_authentication(peer, [(i + 1) as u8; 16]),
                Ok(SessionRegistration::New)
            );
        }
        assert_eq!(sessions.len(), MAX_CONTROL_SESSIONS);
        assert_eq!(
            sessions.activate_after_authentication(
                PeerSessionKey {
                    network: NetworkId::from_bytes([9; 16]),
                    peer: NodeId::from_bytes([9; 16]),
                },
                [0xFF; 16]
            ),
            Err(SessionRegistryError::SessionLimitReached)
        );
        assert_eq!(sessions.len(), MAX_CONTROL_SESSIONS);
    }
}

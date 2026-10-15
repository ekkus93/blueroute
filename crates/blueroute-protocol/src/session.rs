//! Deterministic logical control-session lifecycle and stale-event handling.
//!
//! This module performs no cryptography. Only the future authenticated TLS
//! session layer may call activate_after_authentication; no daemon path uses
//! this registry yet. Never construct an authorized member from this table.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use blueroute_core::{NetworkId, NodeId};

pub const MAX_CONTROL_SESSIONS: usize = 128;
/// Bounded in-process replay horizon for TLS-exported control-session IDs.
/// This is defense in depth, not a substitute for fresh cryptographic exporters.
pub const MAX_RECENT_SESSION_IDS: usize = 4096;

/// Software-only retry policy for a future authenticated control transport.
/// The transport must also impose connect/handshake timeouts and cancellation.
/// Retrying never grants authorization and must not bypass the TLS trust gate.
pub const MAX_RECONNECT_ATTEMPTS: u8 = 8;
/// Maximum time for one connect + mutual TLS + Hello attempt. This is a
/// scheduler watchdog only: the daemon MUST cancel and close the old socket
/// before reporting the attempt failed or starting a replacement.
pub const MAX_CONNECT_AUTH_ATTEMPT_MS: u64 = ConnectionStage::Connecting.timeout_ms()
    + ConnectionStage::TlsHandshake.timeout_ms()
    + ConnectionStage::Hello.timeout_ms();
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

impl PeerSessionKey {
    /// A zero logical identity is never a valid authenticated peer.
    pub fn is_valid(self) -> bool {
        self.network != NetworkId::from_bytes([0; 16]) && self.peer != NodeId::from_bytes([0; 16])
    }
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
    InvalidPeerIdentity,
    ZeroSessionId,
    DuplicateActiveSession,
    SessionLimitReached,
    ReusedSessionId,
}

impl fmt::Display for SessionRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPeerIdentity => f.write_str("invalid zero control-session peer identity"),
            Self::ZeroSessionId => f.write_str("invalid zero session id"),
            Self::DuplicateActiveSession => {
                f.write_str("a different session is already active for this peer")
            }
            Self::SessionLimitReached => f.write_str("control session capacity reached"),
            Self::ReusedSessionId => {
                f.write_str("recently used control session ID cannot be replayed")
            }
        }
    }
}

impl std::error::Error for SessionRegistryError {}

#[derive(Debug, Default)]
pub struct ControlSessionRegistry {
    sessions: BTreeMap<PeerSessionKey, SessionEntry>,
    recent_session_ids: VecDeque<[u8; 16]>,
    seen_session_ids: BTreeSet<[u8; 16]>,
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

    fn remember_session_id(&mut self, session_id: [u8; 16]) {
        if self.recent_session_ids.len() == MAX_RECENT_SESSION_IDS
            && let Some(oldest) = self.recent_session_ids.pop_front()
        {
            self.seen_session_ids.remove(&oldest);
        }
        self.recent_session_ids.push_back(session_id);
        self.seen_session_ids.insert(session_id);
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
        if !key.is_valid() {
            return Err(SessionRegistryError::InvalidPeerIdentity);
        }
        if session_id.iter().all(|byte| *byte == 0) {
            return Err(SessionRegistryError::ZeroSessionId);
        }
        // Exact active re-registration is idempotent; a competing generation
        // cannot replace an active peer, even if the transport reconnects.
        if let Some(existing) = self.sessions.get(&key) {
            if existing.phase == SessionPhase::Active {
                return if existing.session_id == session_id {
                    Ok(SessionRegistration::AlreadyActive)
                } else {
                    Err(SessionRegistryError::DuplicateActiveSession)
                };
            }
            if existing.session_id == session_id {
                return Err(SessionRegistryError::DuplicateActiveSession);
            }
        }
        if self
            .sessions
            .iter()
            .any(|(other, existing)| *other != key && existing.session_id == session_id)
        {
            return Err(SessionRegistryError::DuplicateActiveSession);
        }
        // Keep a bounded replay history across replacement and explicit removal.
        if self.seen_session_ids.contains(&session_id) {
            return Err(SessionRegistryError::ReusedSessionId);
        }
        if let Some(existing) = self.sessions.get_mut(&key) {
            *existing = SessionEntry {
                session_id,
                phase: SessionPhase::Active,
            };
            self.remember_session_id(session_id);
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
        self.remember_session_id(session_id);
        Ok(SessionRegistration::New)
    }

    /// Returns false for a stale loss from a previously superseded connection.
    pub fn mark_lost(&mut self, key: PeerSessionKey, session_id: [u8; 16]) -> bool {
        let Some(existing) = self.sessions.get_mut(&key) else {
            return false;
        };
        // A duplicate disconnect must not schedule another retry for an
        // already-lost session. A genuinely failed reconnect may transition
        // Reconnecting -> Lost to schedule its next bounded attempt.
        if existing.session_id != session_id || existing.phase == SessionPhase::Lost {
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
        // Admit precisely one reconnect attempt for each Lost transition.
        // Repeated wakeups must not fan out concurrent attempts.
        if existing.session_id != session_id || existing.phase != SessionPhase::Lost {
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

/// A deterministic per-peer reconnect scheduler. This does NOT establish
/// authentication or open a socket. The daemon must schedule loss only after
/// matching the currently authenticated session, and must gate every new
/// attempt through mutual TLS, pinning, enrollment and fresh hello validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconnectScheduleError {
    CapacityReached,
    ClockRegression,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RetrySlot {
    session_id: [u8; 16],
    backoff: ReconnectBackoff,
    next_attempt_ms: Option<u64>,
    in_flight: bool,
    in_flight_started_ms: Option<u64>,
    cancel_requested: bool,
    last_ms: u64,
}

#[derive(Debug, Default)]
pub struct ReconnectScheduler {
    slots: BTreeMap<PeerSessionKey, RetrySlot>,
}

impl ReconnectScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Schedule the first attempt exactly once after a verified loss event.
    /// Duplicate and stale notifications cannot reset or shorten backoff.
    pub fn schedule_loss(
        &mut self,
        key: PeerSessionKey,
        session_id: [u8; 16],
        now_ms: u64,
    ) -> Result<bool, ReconnectScheduleError> {
        if !key.is_valid() || session_id.iter().all(|byte| *byte == 0) {
            return Ok(false);
        }
        if let Some(existing) = self.slots.get(&key) {
            if now_ms < existing.last_ms {
                return Err(ReconnectScheduleError::ClockRegression);
            }
            return Ok(false);
        }
        if self.slots.len() >= MAX_CONTROL_SESSIONS {
            return Err(ReconnectScheduleError::CapacityReached);
        }
        let mut backoff = ReconnectBackoff::new();
        let delay = backoff.after_failure().expect("first retry is available");
        self.slots.insert(
            key,
            RetrySlot {
                session_id,
                backoff,
                next_attempt_ms: Some(now_ms.saturating_add(delay.as_millis() as u64)),
                in_flight: false,
                in_flight_started_ms: None,
                cancel_requested: false,
                last_ms: now_ms,
            },
        );
        Ok(true)
    }

    /// Claim a due attempt once. Repeated wakeups cannot create parallel
    /// connection attempts for one logical peer.
    pub fn take_due(
        &mut self,
        key: PeerSessionKey,
        session_id: [u8; 16],
        now_ms: u64,
    ) -> Result<bool, ReconnectScheduleError> {
        let Some(slot) = self.slots.get_mut(&key) else {
            return Ok(false);
        };
        if now_ms < slot.last_ms {
            return Err(ReconnectScheduleError::ClockRegression);
        }
        if slot.session_id != session_id {
            return Ok(false);
        }
        slot.last_ms = now_ms;
        if slot.in_flight || slot.next_attempt_ms.is_none_or(|due| now_ms < due) {
            return Ok(false);
        }
        slot.in_flight = true;
        slot.in_flight_started_ms = Some(now_ms);
        slot.cancel_requested = false;
        slot.next_attempt_ms = None;
        Ok(true)
    }

    /// Return a one-shot cancellation request for an overdue in-flight attempt.
    /// A request NEVER schedules another attempt. The transport owner must
    /// first cancel and close the old connection, then call `attempt_failed`.
    /// This prevents an expired handshake from overlapping its replacement.
    pub fn take_expired_attempt(
        &mut self,
        key: PeerSessionKey,
        session_id: [u8; 16],
        now_ms: u64,
    ) -> Result<bool, ReconnectScheduleError> {
        let Some(slot) = self.slots.get_mut(&key) else {
            return Ok(false);
        };
        if now_ms < slot.last_ms {
            return Err(ReconnectScheduleError::ClockRegression);
        }
        if slot.session_id != session_id {
            return Ok(false);
        }
        slot.last_ms = now_ms;
        let overdue = slot
            .in_flight_started_ms
            .is_some_and(|started| now_ms >= started.saturating_add(MAX_CONNECT_AUTH_ATTEMPT_MS));
        if !slot.in_flight || !overdue || slot.cancel_requested {
            return Ok(false);
        }
        slot.cancel_requested = true;
        Ok(true)
    }

    /// Called once after a failed connection/handshake. Exhausting the retry
    /// budget leaves the entry inert until explicit cancellation/recovery.
    pub fn attempt_failed(
        &mut self,
        key: PeerSessionKey,
        session_id: [u8; 16],
        now_ms: u64,
    ) -> Result<Option<u64>, ReconnectScheduleError> {
        let Some(slot) = self.slots.get_mut(&key) else {
            return Ok(None);
        };
        if now_ms < slot.last_ms {
            return Err(ReconnectScheduleError::ClockRegression);
        }
        if slot.session_id != session_id || !slot.in_flight {
            return Ok(None);
        }
        slot.last_ms = now_ms;
        slot.in_flight = false;
        slot.in_flight_started_ms = None;
        slot.cancel_requested = false;
        slot.next_attempt_ms = slot
            .backoff
            .after_failure()
            .map(|delay| now_ms.saturating_add(delay.as_millis() as u64));
        Ok(slot.next_attempt_ms)
    }

    /// Cancel only the exact retained loss generation. A stale success,
    /// disconnect or shutdown task cannot erase a newer peer's retry state.
    pub fn cancel_if_current(&mut self, key: PeerSessionKey, session_id: [u8; 16]) -> bool {
        if self
            .slots
            .get(&key)
            .is_some_and(|slot| slot.session_id == session_id)
        {
            self.slots.remove(&key);
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
    fn reconnect_scheduler_deduplicates_loss_and_wakeups() {
        let mut scheduler = ReconnectScheduler::new();
        let peer = key(2);
        assert_eq!(scheduler.schedule_loss(peer, [3; 16], 1_000), Ok(true));
        assert_eq!(scheduler.schedule_loss(peer, [3; 16], 1_200), Ok(false));
        assert_eq!(scheduler.schedule_loss(peer, [4; 16], 1_200), Ok(false));
        assert_eq!(scheduler.take_due(peer, [3; 16], 1_249), Ok(false));
        assert_eq!(scheduler.take_due(peer, [4; 16], 1_250), Ok(false));
        assert_eq!(scheduler.take_due(peer, [3; 16], 1_250), Ok(true));
        assert_eq!(scheduler.take_due(peer, [3; 16], 1_250), Ok(false));
        assert_eq!(
            scheduler.attempt_failed(peer, [3; 16], 1_260),
            Ok(Some(1_760))
        );
        assert_eq!(scheduler.attempt_failed(peer, [3; 16], 1_260), Ok(None));
        assert_eq!(scheduler.take_due(peer, [3; 16], 1_759), Ok(false));
        assert_eq!(scheduler.take_due(peer, [3; 16], 1_760), Ok(true));
        assert!(!scheduler.cancel_if_current(peer, [4; 16]));
        assert!(scheduler.cancel_if_current(peer, [3; 16]));
        assert!(!scheduler.cancel_if_current(peer, [3; 16]));
        assert_eq!(scheduler.len(), 0);
    }

    #[test]
    fn overdue_handshake_requests_one_cancel_without_parallel_retry() {
        let mut scheduler = ReconnectScheduler::new();
        let peer = key(2);
        let old = [7; 16];
        assert_eq!(MAX_CONNECT_AUTH_ATTEMPT_MS, 20_000);
        assert_eq!(scheduler.schedule_loss(peer, old, 1_000), Ok(true));
        assert_eq!(scheduler.take_due(peer, old, 1_250), Ok(true));
        assert_eq!(scheduler.take_expired_attempt(peer, old, 21_249), Ok(false));
        assert_eq!(
            scheduler.take_expired_attempt(peer, [8; 16], 21_250),
            Ok(false)
        );
        assert_eq!(scheduler.take_expired_attempt(peer, old, 21_250), Ok(true));
        assert_eq!(scheduler.take_expired_attempt(peer, old, 21_251), Ok(false));
        assert_eq!(scheduler.take_due(peer, old, 50_000), Ok(false));
        assert_eq!(
            scheduler.take_expired_attempt(peer, old, 49_999),
            Err(ReconnectScheduleError::ClockRegression)
        );
        // The transport owner confirms cancellation/teardown before
        // advancing the retry budget; no new attempt was made in parallel.
        assert_eq!(
            scheduler.attempt_failed(peer, old, 50_000),
            Ok(Some(50_500))
        );
        assert_eq!(scheduler.take_expired_attempt(peer, old, 50_500), Ok(false));
        assert_eq!(scheduler.take_due(peer, old, 50_500), Ok(true));
        assert_eq!(scheduler.take_expired_attempt(peer, old, 70_500), Ok(true));
        assert!(scheduler.cancel_if_current(peer, old));
        assert_eq!(scheduler.take_expired_attempt(peer, old, 90_500), Ok(false));
    }

    #[test]
    fn expired_handshake_watchdogs_are_isolated_per_peer_and_generation() {
        let mut scheduler = ReconnectScheduler::new();
        let first = key(2);
        let second = key(3);
        assert_eq!(scheduler.schedule_loss(first, [4; 16], 0), Ok(true));
        assert_eq!(scheduler.schedule_loss(second, [5; 16], 0), Ok(true));
        assert_eq!(scheduler.take_due(first, [4; 16], 250), Ok(true));
        assert_eq!(scheduler.take_due(second, [5; 16], 250), Ok(true));
        assert_eq!(
            scheduler.take_expired_attempt(first, [4; 16], 20_250),
            Ok(true)
        );
        assert_eq!(
            scheduler.take_expired_attempt(second, [5; 16], 20_249),
            Ok(false)
        );
        assert_eq!(
            scheduler.take_expired_attempt(second, [5; 16], 20_250),
            Ok(true)
        );
        assert!(scheduler.cancel_if_current(first, [4; 16]));
        assert_eq!(scheduler.schedule_loss(first, [6; 16], 20_251), Ok(true));
        assert_eq!(
            scheduler.take_expired_attempt(first, [4; 16], 99_999),
            Ok(false)
        );
        assert_eq!(scheduler.take_due(first, [6; 16], 20_501), Ok(true));
        assert_eq!(
            scheduler.take_expired_attempt(first, [6; 16], 40_501),
            Ok(true)
        );
    }

    #[test]
    fn reconnect_scheduler_exhausts_budget_without_spin_or_clock_regression() {
        let mut scheduler = ReconnectScheduler::new();
        let peer = key(2);
        assert_eq!(scheduler.schedule_loss(peer, [7; 16], 0), Ok(true));
        assert_eq!(scheduler.take_due(peer, [7; 16], 100), Ok(false));
        assert_eq!(
            scheduler.take_due(peer, [7; 16], 99),
            Err(ReconnectScheduleError::ClockRegression)
        );
        let mut now = 0_u64;
        for _ in 0..usize::from(MAX_RECONNECT_ATTEMPTS) {
            now += 30_000;
            assert_eq!(scheduler.take_due(peer, [7; 16], now), Ok(true));
            let _ = scheduler.attempt_failed(peer, [7; 16], now + 1).unwrap();
            now += 1;
        }
        assert_eq!(scheduler.take_due(peer, [7; 16], u64::MAX), Ok(false));
        assert_eq!(scheduler.attempt_failed(peer, [7; 16], u64::MAX), Ok(None));
        assert_eq!(scheduler.len(), 1);
    }

    #[test]
    fn reconnect_scheduler_keeps_peer_budgets_independent() {
        let mut scheduler = ReconnectScheduler::new();
        assert_eq!(scheduler.schedule_loss(key(2), [3; 16], 100), Ok(true));
        assert_eq!(scheduler.schedule_loss(key(3), [4; 16], 200), Ok(true));
        assert_eq!(scheduler.take_due(key(2), [3; 16], 350), Ok(true));
        assert_eq!(scheduler.take_due(key(3), [4; 16], 350), Ok(false));
        assert_eq!(scheduler.take_due(key(3), [4; 16], 450), Ok(true));
        assert!(scheduler.cancel_if_current(key(2), [3; 16]));
        assert_eq!(scheduler.len(), 1);
        assert_eq!(scheduler.schedule_loss(key(3), [0; 16], 500), Ok(false));
    }

    #[test]
    fn impossible_peer_identities_never_enter_session_or_reconnect_registry() {
        let mut sessions = ControlSessionRegistry::new();
        let mut retries = ReconnectScheduler::new();
        let invalid_keys = [
            PeerSessionKey {
                network: NetworkId::from_bytes([0; 16]),
                peer: NodeId::from_bytes([2; 16]),
            },
            PeerSessionKey {
                network: NetworkId::from_bytes([1; 16]),
                peer: NodeId::from_bytes([0; 16]),
            },
        ];
        for invalid in invalid_keys {
            assert!(!invalid.is_valid());
            assert_eq!(
                sessions.activate_after_authentication(invalid, [3; 16]),
                Err(SessionRegistryError::InvalidPeerIdentity)
            );
            assert_eq!(retries.schedule_loss(invalid, [3; 16], 0), Ok(false));
        }
        assert!(sessions.is_empty());
        assert!(retries.is_empty());
        assert!(key(2).is_valid());
        assert_eq!(
            sessions.activate_after_authentication(key(2), [3; 16]),
            Ok(SessionRegistration::New)
        );
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
    fn duplicate_loss_and_reconnect_notifications_are_idempotent() {
        let mut registry = ControlSessionRegistry::new();
        let key = key(2);
        registry
            .activate_after_authentication(key, [9; 16])
            .unwrap();
        assert!(registry.mark_lost(key, [9; 16]));
        assert!(!registry.mark_lost(key, [9; 16]));
        assert!(registry.mark_reconnecting(key, [9; 16]));
        assert!(!registry.mark_reconnecting(key, [9; 16]));
        // A failed attempt can return to Lost exactly once.
        assert!(registry.mark_lost(key, [9; 16]));
        assert!(!registry.mark_lost(key, [9; 16]));
        assert!(registry.mark_reconnecting(key, [9; 16]));
        assert!(!registry.mark_reconnecting(key, [8; 16]));
        assert!(!registry.mark_lost(key, [8; 16]));
        // A verified replacement invalidates all stale loss notifications.
        assert_eq!(
            registry.activate_after_authentication(key, [10; 16]),
            Ok(SessionRegistration::Recovered)
        );
        assert!(!registry.mark_lost(key, [9; 16]));
        assert!(!registry.mark_reconnecting(key, [9; 16]));
        assert_eq!(registry.get(key).unwrap().phase, SessionPhase::Active);
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
    fn retired_session_ids_cannot_reenter_after_replacement_or_removal() {
        let mut registry = ControlSessionRegistry::new();
        let peer = key(2);
        assert_eq!(
            registry.activate_after_authentication(peer, [3; 16]),
            Ok(SessionRegistration::New)
        );
        assert!(registry.mark_lost(peer, [3; 16]));
        assert_eq!(
            registry.activate_after_authentication(peer, [4; 16]),
            Ok(SessionRegistration::Recovered)
        );
        assert!(registry.mark_lost(peer, [4; 16]));
        assert_eq!(
            registry.activate_after_authentication(peer, [3; 16]),
            Err(SessionRegistryError::ReusedSessionId)
        );
        assert!(registry.remove_if_current(peer, [4; 16]));
        assert_eq!(
            registry.activate_after_authentication(key(3), [4; 16]),
            Err(SessionRegistryError::ReusedSessionId)
        );
        assert_eq!(
            registry.activate_after_authentication(peer, [5; 16]),
            Ok(SessionRegistration::New)
        );
    }

    #[test]
    fn replay_history_is_bounded_without_leaking_live_session_capacity() {
        let mut registry = ControlSessionRegistry::new();
        let peer = key(2);
        for number in 1..=(MAX_RECENT_SESSION_IDS as u32 + 1) {
            let mut id = [0_u8; 16];
            id[..4].copy_from_slice(&number.to_le_bytes());
            assert_eq!(
                registry.activate_after_authentication(peer, id),
                Ok(SessionRegistration::New)
            );
            assert!(registry.remove_if_current(peer, id));
        }
        assert_eq!(registry.recent_session_ids.len(), MAX_RECENT_SESSION_IDS);
        assert_eq!(registry.seen_session_ids.len(), MAX_RECENT_SESSION_IDS);
        let mut recent = [0_u8; 16];
        recent[..4].copy_from_slice(&(MAX_RECENT_SESSION_IDS as u32 + 1).to_le_bytes());
        assert_eq!(
            registry.activate_after_authentication(peer, recent),
            Err(SessionRegistryError::ReusedSessionId)
        );
        let mut evicted = [0_u8; 16];
        evicted[..4].copy_from_slice(&1_u32.to_le_bytes());
        assert_eq!(
            registry.activate_after_authentication(peer, evicted),
            Ok(SessionRegistration::New)
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
        assert_eq!(deadline.abort(5_000), Err(DeadlineError::ClockRegression));
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
                network: NetworkId::from_bytes([1 + (i / 64) as u8; 16]),
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

//! Deterministic logical control-session lifecycle and stale-event handling.
//!
//! This module performs no cryptography. Only the future authenticated TLS
//! session layer may call activate_after_authentication; no daemon path uses
//! this registry yet. Never construct an authorized member from this table.

use std::collections::BTreeMap;
use std::fmt;

use blueroute_core::{NetworkId, NodeId};

pub const MAX_CONTROL_SESSIONS: usize = 128;

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
        if let Some(existing) = self.sessions.get_mut(&key) {
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
        sessions.activate_after_authentication(key(2), [3; 16]).unwrap();
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
    fn simultaneous_distinct_peers_and_capacity_are_bounded() {
        let mut sessions = ControlSessionRegistry::new();
        for i in 0..MAX_CONTROL_SESSIONS {
            let peer = PeerSessionKey {
                network: NetworkId::from_bytes([(i >> 8) as u8; 16]),
                peer: NodeId::from_bytes([(i + 1) as u8; 16]),
            };
            assert_eq!(
                sessions.activate_after_authentication(peer, [8; 16]),
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
                [8; 16]
            ),
            Err(SessionRegistryError::SessionLimitReached)
        );
        assert_eq!(sessions.len(), MAX_CONTROL_SESSIONS);
    }
}

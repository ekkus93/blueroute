//! P7-004 active registry generation predicate.
use super::*;
use blueroute_protocol::session::SessionPhase;

pub(super) fn registration_is_active(registry: &ControlSessionRegistry, key: PeerSessionKey, session_id: [u8; 16]) -> bool {
    registry.get(key).is_some_and(|entry| entry.session_id == session_id && entry.phase == SessionPhase::Active)
}

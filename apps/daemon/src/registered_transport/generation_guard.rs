//! Reject I/O by a TLS owner whose registry generation is no longer active.
use super::*;
use blueroute_protocol::session::SessionPhase;

impl RegisteredControlSession<'_> {
    pub(super) fn ensure_active_registration(&mut self) -> Result<(), ControlSessionError> {
        // Release the registry lock before closing TCP and retiring the slot.
        let active = self
            .registry
            .lock()
            .is_ok_and(|registry| registration_is_active(&registry, self.key, self.session_id));
        if active && !self.session.is_terminal() {
            return Ok(());
        }
        self.close_and_retire();
        Err(ControlSessionError::Ingress(crate::PinnedIngressError::Terminal))
    }
}

fn registration_is_active(registry: &ControlSessionRegistry, key: PeerSessionKey, session_id: [u8; 16]) -> bool {
    registry.get(key).is_some_and(|entry| entry.session_id == session_id && entry.phase == SessionPhase::Active)
}

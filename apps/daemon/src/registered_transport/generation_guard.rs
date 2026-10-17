//! Reject I/O by a TLS owner whose registry generation is no longer active.
use super::*;
use blueroute_protocol::session::SessionPhase;

impl RegisteredControlSession<'_> {
    /// Fail closed if this exact generation was replaced, lost, or removed.
    /// Release the mutex before shutting down the socket.
    pub(super) fn ensure_current_registration(&mut self) -> Result<(), ControlSessionError> {
        let active = self.registry.lock().is_ok_and(|registry| {
            registration_is_active(&registry, self.key, self.session_id)
        });
        if active {
            return Ok(());
        }
        self.close_and_retire();
        Err(ControlSessionError::Ingress(crate::PinnedIngressError::Terminal))
    }

    pub(super) fn ensure_active_registration(&mut self) -> Result<(), ControlSessionError> {
        self.ensure_current_registration()?;
        if self.session.is_terminal() {
            self.close_and_retire();
            return Err(ControlSessionError::Ingress(
                crate::PinnedIngressError::Terminal,
            ));
        }
        Ok(())
    }
}

fn registration_is_active(
    registry: &ControlSessionRegistry,
    key: PeerSessionKey,
    session_id: [u8; 16],
) -> bool {
    registry.get(key).is_some_and(|entry| {
        entry.session_id == session_id && entry.phase == SessionPhase::Active
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_active_exact_peer_and_generation_may_continue_io() {
        let key = PeerSessionKey {
            network: NetworkId::from_bytes([1; 16]),
            peer: NodeId::from_bytes([2; 16]),
        };
        let other = PeerSessionKey {
            network: NetworkId::from_bytes([3; 16]),
            peer: key.peer,
        };
        let mut registry = ControlSessionRegistry::new();
        assert!(!registration_is_active(&registry, key, [4; 16]));
        registry.activate_after_authentication(key, [4; 16]).unwrap();
        assert!(registration_is_active(&registry, key, [4; 16]));
        assert!(!registration_is_active(&registry, key, [5; 16]));
        assert!(!registration_is_active(&registry, other, [4; 16]));
        assert!(registry.mark_lost(key, [4; 16]));
        assert!(!registration_is_active(&registry, key, [4; 16]));
        assert!(registry.mark_reconnecting(key, [4; 16]));
        registry.activate_after_authentication(key, [5; 16]).unwrap();
        assert!(!registration_is_active(&registry, key, [4; 16]));
        assert!(registration_is_active(&registry, key, [5; 16]));
        assert!(!registry.remove_if_current(key, [4; 16]));
        assert!(registration_is_active(&registry, key, [5; 16]));
    }
}

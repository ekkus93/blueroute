//! Registry-backed ownership of an *already mutually authenticated* TLS session.
//!
//! Only an ActiveControlSession (certificate, exporter and durable pin checks
//! already proven) can enter this registry. No caller-provided identity or
//! session identifier can be substituted into the registration operation.
//! This is a daemon-runtime building block, not a PAN listener or JoinNetwork.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use blueroute_core::{
    CoreError, NapAdmissionDecision, NetworkId, NetworkNapAdmission, NodeId,
};
use blueroute_protocol::session::{
    ControlSessionRegistry, PeerSessionKey, SessionRegistration, SessionRegistryError,
};

mod generation_guard;
#[path = "nap_control_admission.rs"]
mod nap_control_admission;
pub use nap_control_admission::AuthenticatedNapTicket;

use crate::control_tls::ControlSessionRead;
use crate::{ActiveControlSession, ControlSessionError};

#[derive(Debug)]
pub enum SessionAdmissionError {
    Registry(SessionRegistryError),
    LockPoisoned,
}

impl fmt::Display for SessionAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "control session admission rejected: {error}"),
            Self::LockPoisoned => f.write_str("control session registry lock poisoned"),
        }
    }
}

impl std::error::Error for SessionAdmissionError {}

/// The sole owner of a registered, channel-bound TLS transport. On any
/// terminal read failure, orderly EOF or owner drop, the exact generation is
/// removed without affecting a replacement. This type does not authorize a
/// remote membership transition by itself.
pub struct RegisteredControlSession<'a> {
    session: ActiveControlSession<'a>,
    registry: Arc<Mutex<ControlSessionRegistry>>,
    key: PeerSessionKey,
    session_id: [u8; 16],
}

impl<'a> RegisteredControlSession<'a> {
    /// Reserve a NAP slot using this live, mutually authenticated and approved
    /// control session; this does not establish or observe a PANU attachment.
    pub fn reserve_nap(
        &mut self,
        admission: &NetworkNapAdmission,
    ) -> Result<(AuthenticatedNapTicket, NapAdmissionDecision), CoreError> {
        nap_control_admission::reserve_authenticated_nap(self, admission)
    }

    /// Call only after independently observing the matching PANU attachment.
    pub fn activate_nap(
        &mut self,
        admission: &NetworkNapAdmission,
        ticket: &AuthenticatedNapTicket,
    ) -> Result<bool, CoreError> {
        nap_control_admission::activate_authenticated_nap(self, admission, ticket)
    }

    /// Retire an exact reservation even after its TLS stream is gone.
    pub fn retire_nap(
        admission: &NetworkNapAdmission,
        ticket: &AuthenticatedNapTicket,
    ) -> Result<bool, CoreError> {
        nap_control_admission::retire_authenticated_nap(admission, ticket)
    }

    /// Fail closed on poisoned registry state, capacity exhaustion, replay or
    /// another concurrently active session for this logical peer. The session
    /// is dropped (closing its owned socket) on any failure.
    pub fn admit(
        session: ActiveControlSession<'a>,
        registry: Arc<Mutex<ControlSessionRegistry>>,
    ) -> Result<Self, SessionAdmissionError> {
        let key = PeerSessionKey {
            network: session.peer().network(),
            peer: session.peer().node(),
        };
        let session_id = *session.session_id();
        {
            let mut registry_guard = registry
                .lock()
                .map_err(|_| SessionAdmissionError::LockPoisoned)?;
            match registry_guard.activate_after_authentication(key, session_id) {
                // An already-active registration is not a second transport:
                // allowing its duplicate handle would create two owners.
                Ok(SessionRegistration::New | SessionRegistration::Recovered) => {}
                Ok(SessionRegistration::AlreadyActive) => {
                    return Err(SessionAdmissionError::Registry(
                        SessionRegistryError::DuplicateActiveSession,
                    ));
                }
                Err(error) => return Err(SessionAdmissionError::Registry(error)),
            }
        }
        Ok(Self {
            session,
            registry,
            key,
            session_id,
        })
    }

    pub const fn network(&self) -> NetworkId {
        self.key.network
    }

    pub const fn peer(&self) -> NodeId {
        self.key.peer
    }

    pub const fn session_id(&self) -> &[u8; 16] {
        &self.session_id
    }

    pub fn is_terminal(&self) -> bool {
        self.session.is_terminal()
    }

    /// Keep all data-plane activity behind the registered session owner.
    /// Non-terminal Idle and valid frames retain the registration; transport
    /// failure and completed Goodbye/EOF retire it exactly once.
    pub fn receive_next_at(
        &mut self,
        now_ms: u64,
    ) -> Result<ControlSessionRead, ControlSessionError> {
        self.ensure_active_registration()?;
        let result = self.session.receive_next_at(now_ms);
        if result.is_err() || matches!(&result, Ok(ControlSessionRead::Closed)) {
            self.close_and_retire();
        } else {
            // A replacement can be admitted while a read is in flight.
            // Never deliver its predecessor's buffered frames to the daemon.
            self.ensure_active_registration()?;
        }
        result
    }

    pub fn send_heartbeat_until(&mut self, deadline: Instant) -> Result<(), ControlSessionError> {
        self.ensure_active_registration()?;
        let result = self.session.send_heartbeat_until(deadline);
        if result.is_err() {
            self.close_and_retire();
        } else {
            self.ensure_active_registration()?;
        }
        result
    }

    pub fn send_goodbye_until(&mut self, deadline: Instant) -> Result<(), ControlSessionError> {
        self.ensure_active_registration()?;
        let result = self.session.send_goodbye_until(deadline);
        if result.is_err() {
            self.close_and_retire();
        } else {
            // Goodbye makes the stream terminal; only the registry identity
            // (not the stream's terminal bit) is checked after sending.
            self.ensure_current_registration()?;
        }
        result
    }

    /// Recheck durable approval without transmitting or waiting for inbound
    /// traffic. A revoked or terminal peer loses its owned socket before the
    /// exact registry generation is retired. Unrelated peers remain intact.
    pub fn revalidate_approval(&mut self) -> Result<(), ControlSessionError> {
        self.ensure_active_registration()?;
        let result = self.session.revalidate_approval();
        if result.is_err() {
            self.close_and_retire();
        } else {
            self.ensure_active_registration()?;
        }
        result
    }

    /// Explicitly cancel a registered control transport on PAN/link loss,
    /// daemon shutdown or a reconnect-generation change. Shutdown happens
    /// before releasing the registry slot, and repeated cancellation cannot
    /// retire a replacement generation. This never authorizes membership.
    pub fn cancel(&mut self) {
        self.close_and_retire();
    }

    /// Cancel only when the callback refers to this exact TLS exporter
    /// generation. A delayed link-loss event for an old connection must not
    /// shut down its authenticated replacement or retire its registry slot.
    pub fn cancel_if_current(&mut self, expected_session_id: [u8; 16]) -> bool {
        if self.session_id != expected_session_id {
            return false;
        }
        self.close_and_retire();
        true
    }

    /// Ignore callbacks from a different logical peer or TLS generation.
    pub fn cancel_if_current_for_peer(
        &mut self,
        expected_key: PeerSessionKey,
        expected_session_id: [u8; 16],
    ) -> bool {
        if !callback_matches(self.key, self.session_id, expected_key, expected_session_id) {
            return false;
        }
        self.cancel_if_current(expected_session_id)
    }

    /// Always close TCP before releasing the session registry slot.
    fn close_and_retire(&mut self) {
        self.session.close_owned_socket();
        self.retire();
    }

    fn retire(&self) {
        // A poisoned lock denies all new admissions. During teardown we may
        // still remove this exact generation from the poisoned registry,
        // without clearing newer peers. Never panic from Drop.
        let mut guard = match self.registry.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove_if_current(self.key, self.session_id);
    }
}

impl Drop for RegisteredControlSession<'_> {
    fn drop(&mut self) {
        // Shutdown must precede slot removal: otherwise a new authenticated
        // generation could enter while this generation still owns TCP I/O.
        self.close_and_retire();
    }
}

fn callback_matches(
    current_key: PeerSessionKey,
    current_session_id: [u8; 16],
    expected_key: PeerSessionKey,
    expected_session_id: [u8; 16],
) -> bool {
    current_key == expected_key && current_session_id == expected_session_id
}

#[cfg(test)]
mod callback_tests {
    use super::*;

    #[test]
    fn cancellation_callback_requires_both_peer_scope_and_tls_generation() {
        let current = PeerSessionKey {
            network: NetworkId::from_bytes([1; 16]),
            peer: NodeId::from_bytes([2; 16]),
        };
        let wrong_network = PeerSessionKey {
            network: NetworkId::from_bytes([3; 16]),
            peer: current.peer,
        };
        let wrong_peer = PeerSessionKey {
            network: current.network,
            peer: NodeId::from_bytes([4; 16]),
        };
        assert!(callback_matches(current, [5; 16], current, [5; 16]));
        assert!(!callback_matches(current, [5; 16], wrong_network, [5; 16]));
        assert!(!callback_matches(current, [5; 16], wrong_peer, [5; 16]));
        assert!(!callback_matches(current, [5; 16], current, [6; 16]));
    }
}

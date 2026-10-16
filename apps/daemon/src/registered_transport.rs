//! Registry-backed ownership of an *already mutually authenticated* TLS session.
//!
//! Only an ActiveControlSession (certificate, exporter and durable pin checks
//! already proven) can enter this registry. No caller-provided identity or
//! session identifier can be substituted into the registration operation.
//! This is a daemon-runtime building block, not a PAN listener or JoinNetwork.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::session::{
    ControlSessionRegistry, PeerSessionKey, SessionRegistration, SessionRegistryError,
};

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
        let result = self.session.receive_next_at(now_ms);
        if result.is_err() || matches!(&result, Ok(ControlSessionRead::Closed)) {
            self.close_and_retire();
        }
        result
    }

    pub fn send_heartbeat_until(&mut self, deadline: Instant) -> Result<(), ControlSessionError> {
        let result = self.session.send_heartbeat_until(deadline);
        if result.is_err() {
            self.close_and_retire();
        }
        result
    }

    pub fn send_goodbye_until(&mut self, deadline: Instant) -> Result<(), ControlSessionError> {
        let result = self.session.send_goodbye_until(deadline);
        if result.is_err() {
            self.close_and_retire();
        }
        result
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

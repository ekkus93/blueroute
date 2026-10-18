//! P6-007 admission seam requiring an owned, mutually authenticated control session.
//!
//! A caller cannot supply a claimed NodeId to these public functions. The
//! registered session already proved TLS key possession, pinned identity,
//! exporter-bound Hello, and durable approval. Both reservation and activation
//! recheck approval; the daemon must separately observe the actual PANU
//! attachment before activating. This module does not run a PAN listener.
use blueroute_core::{
    CoreError, ErrorKind, NapAdmissionDecision, NetworkId, NetworkNapAdmission, NetworkNapTicket,
    NodeId,
};

use crate::RegisteredControlSession;

/// A NAP reservation bound to the exact TLS exporter session generation.
#[derive(Clone, Debug)]
pub struct AuthenticatedNapTicket {
    ticket: NetworkNapTicket,
    session_id: [u8; 16],
}

impl AuthenticatedNapTicket {
    pub const fn network(&self) -> NetworkId {
        self.ticket.network()
    }

    pub const fn candidate(&self) -> NodeId {
        self.ticket.candidate()
    }
}

/// Owns one capacity reservation until explicitly retired or dropped.
/// Pending leases normally release on drop. Once an attachment is observed,
/// RegisteredNapControlOwner retains capacity on unexpected owner drop until
/// an independently confirmed PANU detach or complete owned NAP shutdown.
#[derive(Debug)]
pub struct AuthenticatedNapLease<'a> {
    admission: &'a NetworkNapAdmission,
    ticket: AuthenticatedNapTicket,
    release_on_drop: bool,
}

impl AuthenticatedNapLease<'_> {
    fn retain_capacity_on_drop(&mut self) {
        self.release_on_drop = false;
    }

    pub fn ticket(&self) -> &AuthenticatedNapTicket {
        &self.ticket
    }

    /// The daemon must independently observe the matching PANU attachment.
    pub fn activate(&self, session: &mut RegisteredControlSession<'_>) -> Result<bool, CoreError> {
        activate_authenticated_nap(session, self.admission, &self.ticket)
    }

    pub fn retire(&self) -> Result<bool, CoreError> {
        retire_authenticated_nap(self.admission, &self.ticket)
    }
}

impl Drop for AuthenticatedNapLease<'_> {
    fn drop(&mut self) {
        // The ledger is generation-scoped. A stale lease can never remove
        // a replacement; on poison, further admission is already denied.
        if self.release_on_drop {
            let _ = self.retire();
        }
    }
}

/// Preferred reservation entry point. The lease releases capacity on every
/// ordinary Rust unwind/drop path, including failed attachment and shutdown.
pub fn reserve_authenticated_nap_lease<'a>(
    session: &mut RegisteredControlSession<'_>,
    admission: &'a NetworkNapAdmission,
) -> Result<(AuthenticatedNapLease<'a>, NapAdmissionDecision), CoreError> {
    let (ticket, decision) = reserve_authenticated_nap(session, admission)?;
    Ok((
        AuthenticatedNapLease {
            admission,
            ticket,
            release_on_drop: true,
        },
        decision,
    ))
}

fn same_session(
    network: NetworkId,
    peer: NodeId,
    session_id: [u8; 16],
    ticket: &AuthenticatedNapTicket,
) -> bool {
    network == ticket.ticket.network()
        && peer == ticket.ticket.candidate()
        && session_id == ticket.session_id
}

fn recheck(session: &mut RegisteredControlSession<'_>) -> Result<(), CoreError> {
    session.revalidate_approval().map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::AuthenticationFailed,
            "NAP admission requires a currently approved authenticated control session",
            error.to_string(),
        )
    })
}

/// Reserve only for a live, registered, approved TLS/Hello peer on this NAP.
/// Capacity reservation is not a PAN attachment or durable membership.
pub fn reserve_authenticated_nap(
    session: &mut RegisteredControlSession<'_>,
    admission: &NetworkNapAdmission,
) -> Result<(AuthenticatedNapTicket, NapAdmissionDecision), CoreError> {
    if session.network() != admission.network() {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "authenticated control session belongs to a different NAP network",
        ));
    }
    recheck(session)?;
    let (ticket, decision) = admission.reserve(session.peer())?;
    Ok((
        AuthenticatedNapTicket {
            ticket,
            session_id: *session.session_id(),
        },
        decision,
    ))
}

/// Activate only after the daemon independently observes the physical PANU
/// attachment and calls this with the *same* still-approved TLS generation.
/// A replacement session cannot activate a predecessor's reservation.
pub fn activate_authenticated_nap(
    session: &mut RegisteredControlSession<'_>,
    admission: &NetworkNapAdmission,
    ticket: &AuthenticatedNapTicket,
) -> Result<bool, CoreError> {
    if !same_session(
        session.network(),
        session.peer(),
        *session.session_id(),
        ticket,
    ) {
        return Err(CoreError::new(
            ErrorKind::AuthenticationFailed,
            "NAP reservation belongs to a different authenticated session",
        ));
    }
    recheck(session)?;
    admission.activate(&ticket.ticket)
}

/// Owns an authenticated TLS session and its matching NAP reservation as one
/// lifecycle unit. The running daemon must supply independent PANU attachment
/// and detachment observations; this type does not discover Bluetooth links.
///
/// Critically, TLS loss does NOT release an already-activated radio capacity
/// slot. A real PANU may remain attached after its control socket closes.
/// Keep this owner until the matching PANU detach has been observed, or until
/// the entire owned NAP has been shut down and its links removed.
#[must_use = "retain the NAP owner until PANU detach or NAP shutdown"]
pub struct RegisteredNapControlOwner<'session, 'nap> {
    session: RegisteredControlSession<'session>,
    lease: Option<AuthenticatedNapLease<'nap>>,
    attached: bool,
}

fn release_pending_on_transport_loss(
    lease: &mut Option<AuthenticatedNapLease<'_>>,
    attached: bool,
) {
    if !attached {
        lease.take();
    }
}

fn preserve_attached_capacity_on_owner_drop(
    lease: &mut Option<AuthenticatedNapLease<'_>>,
    attached: bool,
) {
    if attached && let Some(lease) = lease.as_mut() {
        lease.retain_capacity_on_drop();
    }
}

fn retire_observed_detach_if_current(
    lease: &mut Option<AuthenticatedNapLease<'_>>,
    attached: &mut bool,
    current_session_id: [u8; 16],
    expected_session_id: [u8; 16],
) -> bool {
    if current_session_id != expected_session_id {
        return false;
    }
    *attached = false;
    lease.take();
    true
}

impl<'session, 'nap> RegisteredNapControlOwner<'session, 'nap> {
    /// Consume an already-registered, pinned, durably approved TLS session.
    /// A failed reservation closes the session via its normal owner drop.
    pub fn reserve(
        mut session: RegisteredControlSession<'session>,
        admission: &'nap NetworkNapAdmission,
    ) -> Result<(Self, NapAdmissionDecision), CoreError> {
        let (lease, decision) = session.reserve_nap_lease(admission)?;
        Ok((
            Self {
                session,
                lease: Some(lease),
                attached: false,
            },
            decision,
        ))
    }

    pub const fn network(&self) -> NetworkId {
        self.session.network()
    }

    pub const fn peer(&self) -> NodeId {
        self.session.peer()
    }

    pub const fn session_id(&self) -> &[u8; 16] {
        self.session.session_id()
    }

    /// Must be called only after the daemon independently observes the
    /// matching authenticated peer's PANU attachment. A stale or revoked
    /// reservation cannot be promoted.
    pub fn activate_after_observed_attachment(&mut self) -> Result<(), CoreError> {
        let Some(lease) = &self.lease else {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP reservation was already retired",
            ));
        };
        // The PANU is already independently observed by the caller. Even
        // if policy revalidation or ledger activation now fails, its radio
        // slot cannot be freed until detach is independently confirmed.
        // Repeated calls must retry activation, not mistake this observation
        // for a successfully activated ledger ticket.
        self.attached = true;
        match lease.activate(&mut self.session) {
            Ok(true) => Ok(())
            Ok(false) => {
                self.on_transport_loss();
                Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP reservation was replaced or retired before attachment",
                ))
            }
            Err(error) => {
                self.on_transport_loss();
                Err(error)
            }
        }
    }

    /// A closed or failed TLS stream releases only an unactivated reservation.
    /// An active PANU continues consuming capacity until confirmed detached.
    pub fn on_transport_loss(&mut self) {
        self.session.cancel();
        release_pending_on_transport_loss(&mut self.lease, self.attached);
    }

    /// The caller must have independently observed PANU detachment (or
    /// completed owned NAP shutdown) *and* mapped that event to this exact
    /// TLS generation. Stale callbacks must never retire a replacement.
    /// Close the TLS socket before releasing its capacity reservation.
    pub fn confirm_observed_detach_if_current(&mut self, expected_session_id: [u8; 16]) -> bool {
        if !self.session.cancel_if_current(expected_session_id) {
            return false;
        }
        retire_observed_detach_if_current(
            &mut self.lease,
            &mut self.attached,
            *self.session.session_id(),
            expected_session_id,
        )
    }

    /// A stale TLS callback cannot close a replacement or free its slot.
    pub fn cancel_if_current(&mut self, expected_session_id: [u8; 16]) -> bool {
        if !self.session.cancel_if_current(expected_session_id) {
            return false;
        }
        release_pending_on_transport_loss(&mut self.lease, self.attached);
        true
    }

    pub fn receive_next_at(
        &mut self,
        now_ms: u64,
    ) -> Result<crate::control_tls::ControlSessionRead, crate::ControlSessionError> {
        let result = self.session.receive_next_at(now_ms);
        if result.is_err() || matches!(&result, Ok(crate::control_tls::ControlSessionRead::Closed))
        {
            self.on_transport_loss();
        }
        result
    }

    pub fn revalidate_approval(&mut self) -> Result<(), crate::ControlSessionError> {
        let result = self.session.revalidate_approval();
        if result.is_err() {
            self.on_transport_loss();
        }
        result
    }

    pub fn send_heartbeat_until(
        &mut self,
        deadline: std::time::Instant,
    ) -> Result<(), crate::ControlSessionError> {
        let result = self.session.send_heartbeat_until(deadline);
        if result.is_err() {
            self.on_transport_loss();
        }
        result
    }

    pub fn send_goodbye_until(
        &mut self,
        deadline: std::time::Instant,
    ) -> Result<(), crate::ControlSessionError> {
        let result = self.session.send_goodbye_until(deadline);
        self.on_transport_loss();
        result
    }

    /// Copy the exact admission generation for later independently observed
    /// detachment. The caller must keep this ticket if it cannot retain the
    /// owner; dropping an attached owner conservatively preserves capacity.
    /// Only retire the ticket after matching PANU detach or owned NAP shutdown.
    pub fn observed_attachment_ticket(&self) -> Option<AuthenticatedNapTicket> {
        if self.attached {
            self.lease.as_ref().map(|lease| lease.ticket().clone())
        } else {
            None
        }
    }

    pub const fn attachment_observed(&self) -> bool {
        self.attached
    }

    pub fn awaiting_detach_after_tls_loss(&self) -> bool {
        self.attached && self.session.is_terminal()
    }
}

impl Drop for RegisteredNapControlOwner<'_, '_> {
    fn drop(&mut self) {
        // TLS owner drop is not independent proof of PANU detachment.
        // Preserve occupied capacity rather than admitting an overlapping peer.
        preserve_attached_capacity_on_owner_drop(&mut self.lease, self.attached);
    }
}

/// Teardown callbacks do not require a live TLS stream. Exact NAP owner and
/// reservation generation checks prevent a stale callback from retiring a
/// replacement. This never disconnects the underlying PANU socket.
pub fn retire_authenticated_nap(
    admission: &NetworkNapAdmission,
    ticket: &AuthenticatedNapTicket,
) -> Result<bool, CoreError> {
    admission.retire(&ticket.ticket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::{CapabilitySource, NodeCapabilities, Sourced};

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn peer(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn capabilities() -> NodeCapabilities {
        NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Discovered)),
            nap: Some(Sourced::new(true, CapabilitySource::Measured)),
            connection_policy_ceiling: Some(Sourced::new(2, CapabilitySource::Configured)),
            ..NodeCapabilities::default()
        }
    }

    #[test]
    fn tls_generation_and_peer_scope_cannot_be_substituted() {
        let admission = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (ticket, _) = admission.reserve(peer(2)).unwrap();
        let authenticated = AuthenticatedNapTicket {
            ticket,
            session_id: [7; 16],
        };
        assert!(same_session(network(1), peer(2), [7; 16], &authenticated));
        assert!(!same_session(network(1), peer(2), [8; 16], &authenticated));
        assert!(!same_session(network(1), peer(3), [7; 16], &authenticated));
        assert!(!same_session(network(3), peer(2), [7; 16], &authenticated));
        assert!(retire_authenticated_nap(&admission, &authenticated).unwrap());
        assert!(!retire_authenticated_nap(&admission, &authenticated).unwrap());
    }

    #[test]
    fn scoped_lease_drop_releases_pending_and_active_capacity() {
        let admission = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (pending, _) = admission.reserve(peer(2)).unwrap();
        let lease = AuthenticatedNapLease {
            admission: &admission,
            release_on_drop: true,
            ticket: AuthenticatedNapTicket {
                ticket: pending,
                session_id: [7; 16],
            },
        };
        assert_eq!(admission.counts().unwrap(), (0, 1));
        drop(lease);
        assert_eq!(admission.counts().unwrap(), (0, 0));
        let (active, _) = admission.reserve(peer(2)).unwrap();
        assert!(admission.activate(&active).unwrap());
        let lease = AuthenticatedNapLease {
            admission: &admission,
            release_on_drop: true,
            ticket: AuthenticatedNapTicket {
                ticket: active,
                session_id: [8; 16],
            },
        };
        assert_eq!(admission.counts().unwrap(), (1, 0));
        assert!(lease.retire().unwrap());
        drop(lease);
        assert_eq!(admission.counts().unwrap(), (0, 0));
    }

    #[test]
    fn pending_tls_loss_releases_slot_but_attached_tls_loss_retains_capacity() {
        let admission = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (pending, _) = admission.reserve(peer(2)).unwrap();
        let mut lease = Some(AuthenticatedNapLease {
            admission: &admission,
            release_on_drop: true,
            ticket: AuthenticatedNapTicket {
                ticket: pending,
                session_id: [7; 16],
            },
        });
        release_pending_on_transport_loss(&mut lease, false);
        assert!(lease.is_none());
        assert_eq!(admission.counts().unwrap(), (0, 0));

        let (ticket, _) = admission.reserve(peer(2)).unwrap();
        assert!(admission.activate(&ticket).unwrap());
        let mut lease = Some(AuthenticatedNapLease {
            admission: &admission,
            release_on_drop: true,
            ticket: AuthenticatedNapTicket {
                ticket,
                session_id: [8; 16],
            },
        });
        release_pending_on_transport_loss(&mut lease, true);
        assert!(lease.is_some());
        assert_eq!(admission.counts().unwrap(), (1, 0));
        // Only independently observed PANU detach (or full NAP shutdown)
        // may release an active reservation after the TLS owner disappears.
        lease.take();
        assert_eq!(admission.counts().unwrap(), (0, 0));
    }

    #[test]
    fn owner_drop_preserves_observed_attachment_until_exact_detach() {
        let admission = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (ticket, _) = admission.reserve(peer(2)).unwrap();
        assert!(admission.activate(&ticket).unwrap());
        let authenticated = AuthenticatedNapTicket {
            ticket,
            session_id: [7; 16],
        };
        let mut lease = Some(AuthenticatedNapLease {
            admission: &admission,
            ticket: authenticated.clone(),
            release_on_drop: true,
        });
        preserve_attached_capacity_on_owner_drop(&mut lease, true);
        drop(lease);
        assert_eq!(admission.counts().unwrap(), (1, 0));
        assert!(retire_authenticated_nap(&admission, &authenticated).unwrap());
        let (replacement, _) = admission.reserve(peer(2)).unwrap();
        assert!(!retire_authenticated_nap(&admission, &authenticated).unwrap());
        assert_eq!(admission.counts().unwrap(), (0, 1));
        assert!(admission.retire(&replacement).unwrap());
    }

    #[test]
    fn observed_panu_with_failed_activation_keeps_pending_slot_until_detach() {
        let admission = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (ticket, _) = admission.reserve(peer(2)).unwrap();
        let authenticated = AuthenticatedNapTicket {
            ticket,
            session_id: [7; 16],
        };
        let mut lease = Some(AuthenticatedNapLease {
            admission: &admission,
            ticket: authenticated.clone(),
            release_on_drop: true,
        });
        // A physical attachment was observed before the policy gate failed.
        // TLS loss cannot prove that the PANU disappeared.
        release_pending_on_transport_loss(&mut lease, true);
        assert_eq!(admission.counts().unwrap(), (0, 1));
        preserve_attached_capacity_on_owner_drop(&mut lease, true);
        drop(lease);
        assert_eq!(admission.counts().unwrap(), (0, 1));
        assert!(retire_authenticated_nap(&admission, &authenticated).unwrap());
        assert_eq!(admission.counts().unwrap(), (0, 0));
    }

    #[test]
    fn stale_detach_callback_cannot_retire_replacement_generation() {
        let admission = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (ticket, _) = admission.reserve(peer(2)).unwrap();
        assert!(admission.activate(&ticket).unwrap());
        let mut lease = Some(AuthenticatedNapLease {
            admission: &admission,
            ticket: AuthenticatedNapTicket {
                ticket,
                session_id: [9; 16],
            },
            release_on_drop: true,
        });
        let mut attached = true;
        assert!(!retire_observed_detach_if_current(
            &mut lease,
            &mut attached,
            [9; 16],
            [8; 16],
        ));
        assert!(attached);
        assert!(lease.is_some());
        assert_eq!(admission.counts().unwrap(), (1, 0));
        assert!(retire_observed_detach_if_current(
            &mut lease,
            &mut attached,
            [9; 16],
            [9; 16],
        ));
        assert!(!attached);
        assert!(lease.is_none());
        assert_eq!(admission.counts().unwrap(), (0, 0));
    }

    #[test]
    fn unobserved_attachment_does_not_strand_pending_capacity_on_owner_drop() {
        let admission = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (ticket, _) = admission.reserve(peer(2)).unwrap();
        let mut lease = Some(AuthenticatedNapLease {
            admission: &admission,
            ticket: AuthenticatedNapTicket {
                ticket,
                session_id: [7; 16],
            },
            release_on_drop: true,
        });
        preserve_attached_capacity_on_owner_drop(&mut lease, false);
        drop(lease);
        assert_eq!(admission.counts().unwrap(), (0, 0));
    }

    #[test]
    fn old_owner_cannot_retire_restarted_nap_ticket() {
        let old = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let new = NetworkNapAdmission::new(network(1), capabilities()).unwrap();
        let (ticket, _) = old.reserve(peer(2)).unwrap();
        let old_ticket = AuthenticatedNapTicket {
            ticket,
            session_id: [7; 16],
        };
        let (fresh, _) = new.reserve(peer(2)).unwrap();
        assert!(!retire_authenticated_nap(&new, &old_ticket).unwrap());
        assert!(new.activate(&fresh).unwrap());
        assert_eq!(new.counts().unwrap(), (1, 0));
    }
}

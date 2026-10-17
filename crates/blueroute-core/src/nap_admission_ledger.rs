//! Generation-scoped in-process NAP admission reservations (P6-007).
//!
//! Only pass independently authenticated, network-approved NodeIds to this
//! ledger. Reserving capacity does not authorize membership or connect PANU.
//! The owning daemon must retain one ledger per NAP/network, refresh policy on
//! adapter changes, and reconcile actual attachments after restart.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::{
    CoreError, ErrorKind, NapAdmissionDecision, NodeCapabilities, NodeId, plan_nap_client_admission,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NapAdmissionTicket {
    pub candidate: NodeId,
    pub generation: u64,
}

#[derive(Debug)]
struct State {
    host: NodeCapabilities,
    active: BTreeMap<NodeId, u64>,
    pending: BTreeMap<NodeId, u64>,
    next_generation: u64,
    closed: bool,
}

/// A local reservation ledger that serializes competing PANU admissions.
/// Tickets prevent delayed teardown from retiring a replacement connection.
#[derive(Debug)]
pub struct NapAdmissionLedger {
    state: Mutex<State>,
}

impl NapAdmissionLedger {
    pub fn new(host: NodeCapabilities) -> Self {
        Self {
            state: Mutex::new(State {
                host,
                active: BTreeMap::new(),
                pending: BTreeMap::new(),
                next_generation: 0,
                closed: false,
            }),
        }
    }

    /// Permanently retire this NAP instance. A restarted NAP must construct a
    /// fresh ledger; old callbacks cannot resurrect reservations. Closing is
    /// serialized with admission and activation, so no new admission can win
    /// after this call returns. This does not disconnect physical PAN clients.
    pub fn close(&self) -> Result<(), CoreError> {
        let mut state = self.state.lock().map_err(|_| {
            CoreError::new(ErrorKind::InvalidState, "NAP admission ledger lock poisoned")
        })?;
        state.closed = true;
        state.active.clear();
        state.pending.clear();
        Ok(())
    }

    /// Atomically replace the sourced local adapter/NAP policy under the same
    /// lock as reservations. Existing tickets are retained, even if a new
    /// ceiling is below current occupancy: the owner must separately cancel
    /// or reconcile real connections. No new slot is granted while the
    /// observed reservation count violates the updated policy.
    pub fn update_capabilities(&self, host: NodeCapabilities) -> Result<(), CoreError> {
        let mut state = self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission ledger lock poisoned",
            )
        })?;
        if state.closed {
            return Err(CoreError::new(ErrorKind::InvalidState, "NAP admission owner is closed"));
        }
        state.host = host;
        Ok(())
    }

    /// Atomically reserve one slot against active *and pending* peers.
    ///
    /// A duplicate pending attempt is rejected rather than starting a second
    /// connection. An already active peer returns its original ticket.
    pub fn reserve(
        &self,
        candidate: NodeId,
    ) -> Result<(NapAdmissionTicket, NapAdmissionDecision), CoreError> {
        let mut state = self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission ledger lock poisoned",
            )
        })?;
        if state.closed {
            return Err(CoreError::new(ErrorKind::InvalidState, "NAP admission owner is closed"));
        }
        if state.pending.contains_key(&candidate) {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP peer admission already pending",
            ));
        }
        let observed: Vec<NodeId> = state
            .active
            .keys()
            .chain(state.pending.keys())
            .copied()
            .collect();
        let decision = plan_nap_client_admission(&state.host, &observed, candidate)?;
        if let Some(&generation) = state.active.get(&candidate) {
            return Ok((
                NapAdmissionTicket {
                    candidate,
                    generation,
                },
                decision,
            ));
        }
        let generation = state.next_generation.checked_add(1).ok_or_else(|| {
            CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission generation exhausted",
            )
        })?;
        state.next_generation = generation;
        state.pending.insert(candidate, generation);
        Ok((
            NapAdmissionTicket {
                candidate,
                generation,
            },
            decision,
        ))
    }

    /// Promote only the matching reservation after external PANU authentication
    /// and attachment succeed. A repeated confirmation is idempotent.
    pub fn activate(&self, ticket: NapAdmissionTicket) -> Result<bool, CoreError> {
        let mut state = self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission ledger lock poisoned",
            )
        })?;
        if state.closed {
            return Err(CoreError::new(ErrorKind::InvalidState, "NAP admission owner is closed"));
        }
        if state.active.get(&ticket.candidate) == Some(&ticket.generation) {
            return Ok(false);
        }
        if state.pending.get(&ticket.candidate) != Some(&ticket.generation) {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission ticket is stale or unknown",
            ));
        }
        // A reservation may have been issued before an adapter loss or a
        // policy downgrade. Revalidate under the same lock immediately before
        // promotion; a stale capability snapshot must not authorize activation.
        let observed: Vec<NodeId> = state
            .active
            .keys()
            .chain(state.pending.keys())
            .copied()
            .collect();
        plan_nap_client_admission(&state.host, &observed, ticket.candidate)?;
        state.pending.remove(&ticket.candidate);
        state.active.insert(ticket.candidate, ticket.generation);
        Ok(true)
    }

    /// Retire an exact pending/active generation; stale callbacks are harmless.
    pub fn retire(&self, ticket: NapAdmissionTicket) -> Result<bool, CoreError> {
        let mut state = self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission ledger lock poisoned",
            )
        })?;
        if state.pending.get(&ticket.candidate) == Some(&ticket.generation) {
            state.pending.remove(&ticket.candidate);
            return Ok(true);
        }
        if state.active.get(&ticket.candidate) == Some(&ticket.generation) {
            state.active.remove(&ticket.candidate);
            return Ok(true);
        }
        Ok(false)
    }

    /// In-process counts, not observations of live Bluetooth attachments.
    pub fn counts(&self) -> Result<(usize, usize), CoreError> {
        let state = self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission ledger lock poisoned",
            )
        })?;
        Ok((state.active.len(), state.pending.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use crate::{CapabilitySource, Sourced};

    fn peer(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn host(ceiling: Option<u16>) -> NodeCapabilities {
        NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Discovered)),
            nap: Some(Sourced::new(true, CapabilitySource::Measured)),
            connection_policy_ceiling: ceiling
                .map(|value| Sourced::new(value, CapabilitySource::Configured)),
            ..NodeCapabilities::default()
        }
    }

    #[test]
    fn pending_attempts_consume_capacity_before_attachment() {
        let ledger = NapAdmissionLedger::new(host(Some(1)));
        let (ticket, decision) = ledger.reserve(peer(1)).unwrap();
        assert_eq!(decision.remaining_after, 0);
        assert_eq!(ledger.counts().unwrap(), (0, 1));
        assert_eq!(
            ledger.reserve(peer(2)).unwrap_err().kind(),
            ErrorKind::CapabilityUnavailable
        );
        assert_eq!(
            ledger.reserve(peer(1)).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert!(ledger.activate(ticket).unwrap());
        assert!(!ledger.activate(ticket).unwrap());
        assert_eq!(ledger.counts().unwrap(), (1, 0));
        let (again, decision) = ledger.reserve(peer(1)).unwrap();
        assert_eq!(again, ticket);
        assert!(decision.already_attached);
        assert!(ledger.retire(ticket).unwrap());
        assert!(!ledger.retire(ticket).unwrap());
        assert_eq!(ledger.counts().unwrap(), (0, 0));
    }

    #[test]
    fn old_teardown_cannot_remove_new_generation() {
        let ledger = NapAdmissionLedger::new(host(Some(1)));
        let (old, _) = ledger.reserve(peer(1)).unwrap();
        assert!(ledger.retire(old).unwrap());
        let (new, _) = ledger.reserve(peer(1)).unwrap();
        assert_ne!(old.generation, new.generation);
        assert!(!ledger.retire(old).unwrap());
        assert_eq!(
            ledger.activate(old).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert!(ledger.activate(new).unwrap());
        assert!(!ledger.retire(old).unwrap());
        assert_eq!(ledger.counts().unwrap(), (1, 0));
    }

    #[test]
    fn competing_threads_never_overbook_pending_slots() {
        const WORKERS: usize = 16;
        let ledger = Arc::new(NapAdmissionLedger::new(host(Some(3))));
        let barrier = Arc::new(Barrier::new(WORKERS));
        let handles: Vec<_> = (1..=WORKERS)
            .map(|index| {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    ledger.reserve(peer(index as u8))
                })
            })
            .collect();
        let admitted: Vec<_> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap().ok().map(|(ticket, _)| ticket))
            .collect();
        assert_eq!(admitted.len(), 3);
        assert_eq!(ledger.counts().unwrap(), (0, 3));
        for ticket in admitted {
            assert!(ledger.activate(ticket).unwrap());
        }
        assert_eq!(ledger.counts().unwrap(), (3, 0));
    }

    #[test]
    fn live_policy_downgrade_is_atomic_and_preserves_existing_tickets() {
        let ledger = NapAdmissionLedger::new(host(Some(2)));
        let (active, _) = ledger.reserve(peer(1)).unwrap();
        assert!(ledger.activate(active).unwrap());
        let (pending, _) = ledger.reserve(peer(2)).unwrap();

        ledger.update_capabilities(host(Some(1))).unwrap();
        assert_eq!(ledger.counts().unwrap(), (1, 1));
        assert_eq!(
            ledger.reserve(peer(3)).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert!(ledger.retire(pending).unwrap());
        assert_eq!(
            ledger.reserve(peer(3)).unwrap_err().kind(),
            ErrorKind::CapabilityUnavailable
        );
        assert!(ledger.retire(active).unwrap());

        let (fresh, _) = ledger.reserve(peer(3)).unwrap();
        assert!(ledger.activate(fresh).unwrap());
        ledger.update_capabilities(host(None)).unwrap();
        assert!(ledger.reserve(peer(4)).is_err());
        assert_eq!(ledger.counts().unwrap(), (1, 0));
        assert!(ledger.retire(fresh).unwrap());
        ledger.update_capabilities(host(Some(2))).unwrap();
        assert!(ledger.reserve(peer(4)).is_ok());
    }

    #[test]
    fn pending_activation_revalidates_capabilities_after_reservation() {
        let ledger = NapAdmissionLedger::new(host(Some(2)));
        let (first, _) = ledger.reserve(peer(1)).unwrap();
        let (second, _) = ledger.reserve(peer(2)).unwrap();

        ledger.update_capabilities(host(Some(1))).unwrap();
        assert_eq!(
            ledger.activate(first).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(ledger.counts().unwrap(), (0, 2));
        assert!(ledger.retire(second).unwrap());
        assert!(ledger.activate(first).unwrap());

        let (third, _) = {
            ledger.update_capabilities(host(Some(2))).unwrap();
            ledger.reserve(peer(3)).unwrap()
        };
        ledger.update_capabilities(host(None)).unwrap();
        assert_eq!(
            ledger.activate(third).unwrap_err().kind(),
            ErrorKind::CapabilityUnavailable
        );
        assert_eq!(ledger.counts().unwrap(), (1, 1));
        ledger.update_capabilities(host(Some(2))).unwrap();
        assert!(ledger.activate(third).unwrap());
        assert_eq!(ledger.counts().unwrap(), (2, 0));
    }

    #[test]
    fn close_is_terminal_and_invalidates_all_pending_and_active_tickets() {
        let ledger = NapAdmissionLedger::new(host(Some(3)));
        let (active, _) = ledger.reserve(peer(1)).unwrap();
        assert!(ledger.activate(active).unwrap());
        let (pending, _) = ledger.reserve(peer(2)).unwrap();
        ledger.close().unwrap();
        ledger.close().unwrap();
        assert_eq!(ledger.counts().unwrap(), (0, 0));
        assert!(!ledger.retire(active).unwrap());
        assert!(!ledger.retire(pending).unwrap());
        for ticket in [active, pending] {
            assert_eq!(ledger.activate(ticket).unwrap_err().kind(), ErrorKind::InvalidState);
        }
        assert_eq!(ledger.reserve(peer(3)).unwrap_err().kind(), ErrorKind::InvalidState);
        assert_eq!(ledger.update_capabilities(host(Some(3))).unwrap_err().kind(), ErrorKind::InvalidState);
    }

    #[test]
    fn concurrent_close_cannot_be_undone_by_admission_or_activation() {
        const WORKERS: usize = 16;
        let ledger = Arc::new(NapAdmissionLedger::new(host(Some(WORKERS as u16))));
        let barrier = Arc::new(Barrier::new(WORKERS + 1));
        let handles: Vec<_> = (1..=WORKERS).map(|index| {
            let ledger = Arc::clone(&ledger);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                if let Ok((ticket, _)) = ledger.reserve(peer(index as u8)) {
                    let _ = ledger.activate(ticket);
                }
            })
        }).collect();
        barrier.wait();
        ledger.close().unwrap();
        for handle in handles { handle.join().unwrap(); }
        assert_eq!(ledger.counts().unwrap(), (0, 0));
        assert!(ledger.reserve(peer(20)).is_err());
    }

    #[test]
    fn unknown_or_disabled_policy_does_not_reserve() {
        for host in [host(None), host(Some(0))] {
            let ledger = NapAdmissionLedger::new(host);
            assert!(ledger.reserve(peer(1)).is_err());
            assert_eq!(ledger.counts().unwrap(), (0, 0));
        }
        let ledger = NapAdmissionLedger::new(host(Some(2)));
        assert!(ledger.reserve(peer(0)).is_err());
        assert_eq!(ledger.counts().unwrap(), (0, 0));
    }
}

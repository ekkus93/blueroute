//! Generation-scoped in-process NAP admission reservations (P6-007).
//!
//! Only pass independently authenticated, network-approved NodeIds to this
//! ledger. Reserving capacity does not authorize membership or connect PANU.
//! The owning daemon must retain one ledger per NAP/network, refresh policy on
//! adapter changes, and reconcile actual attachments after restart.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::{
    CoreError, ErrorKind, NapAdmissionDecision, NodeCapabilities, NodeId,
    plan_nap_client_admission,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NapAdmissionTicket {
    pub candidate: NodeId,
    pub generation: u64,
}

#[derive(Debug, Default)]
struct State {
    active: BTreeMap<NodeId, u64>,
    pending: BTreeMap<NodeId, u64>,
    next_generation: u64,
}

/// A local reservation ledger that serializes competing PANU admissions.
/// Tickets prevent delayed teardown from retiring a replacement connection.
#[derive(Debug)]
pub struct NapAdmissionLedger {
    host: NodeCapabilities,
    state: Mutex<State>,
}

impl NapAdmissionLedger {
    pub fn new(host: NodeCapabilities) -> Self {
        Self {
            host,
            state: Mutex::new(State::default()),
        }
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
            CoreError::new(ErrorKind::InvalidState, "NAP admission ledger lock poisoned")
        })?;
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
        let decision = plan_nap_client_admission(&self.host, &observed, candidate)?;
        if let Some(&generation) = state.active.get(&candidate) {
            return Ok((
                NapAdmissionTicket { candidate, generation },
                decision,
            ));
        }
        let generation = state.next_generation.checked_add(1).ok_or_else(|| {
            CoreError::new(ErrorKind::InvalidState, "NAP admission generation exhausted")
        })?;
        state.next_generation = generation;
        state.pending.insert(candidate, generation);
        Ok((
            NapAdmissionTicket { candidate, generation },
            decision,
        ))
    }

    /// Promote only the matching reservation after external PANU authentication
    /// and attachment succeed. A repeated confirmation is idempotent.
    pub fn activate(&self, ticket: NapAdmissionTicket) -> Result<bool, CoreError> {
        let mut state = self.state.lock().map_err(|_| {
            CoreError::new(ErrorKind::InvalidState, "NAP admission ledger lock poisoned")
        })?;
        if state.active.get(&ticket.candidate) == Some(&ticket.generation) {
            return Ok(false);
        }
        if state.pending.get(&ticket.candidate) != Some(&ticket.generation) {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP admission ticket is stale or unknown",
            ));
        }
        state.pending.remove(&ticket.candidate);
        state.active.insert(ticket.candidate, ticket.generation);
        Ok(true)
    }

    /// Retire an exact pending/active generation; stale callbacks are harmless.
    pub fn retire(&self, ticket: NapAdmissionTicket) -> Result<bool, CoreError> {
        let mut state = self.state.lock().map_err(|_| {
            CoreError::new(ErrorKind::InvalidState, "NAP admission ledger lock poisoned")
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
            CoreError::new(ErrorKind::InvalidState, "NAP admission ledger lock poisoned")
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
        assert_eq!(ledger.activate(old).unwrap_err().kind(), ErrorKind::InvalidState);
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

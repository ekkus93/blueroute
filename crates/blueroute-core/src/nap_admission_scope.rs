//! Network- and instance-scoped NAP admission ownership (P6-007).
//!
//! An independent authenticated runtime must establish peer identity and
//! network approval before calling reserve. This module does not authorize
//! peers, observe Bluetooth attachments, or own a PAN socket.
use std::sync::Arc;

use crate::{
    CoreError, ErrorKind, NapAdmissionDecision, NapAdmissionLedger, NapAdmissionTicket, NetworkId,
    NodeCapabilities, NodeId,
};

/// Opaque reservation tied to exactly one NAP owner, even across restarts.
#[derive(Clone, Debug)]
pub struct NetworkNapTicket {
    network: NetworkId,
    owner: Arc<()>,
    inner: NapAdmissionTicket,
}

impl NetworkNapTicket {
    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn candidate(&self) -> NodeId {
        self.inner.candidate
    }

    pub const fn generation(&self) -> u64 {
        self.inner.generation
    }
}

/// One ledger per logical NAP/network. Ownership is not inferred from
/// predictable peer/generation numbers: a private Arc token scopes callbacks
/// to this exact instance and cannot be reconstructed by external callers.
#[derive(Debug)]
pub struct NetworkNapAdmission {
    network: NetworkId,
    owner: Arc<()>,
    ledger: NapAdmissionLedger,
}

impl NetworkNapAdmission {
    pub fn new(network: NetworkId, host: NodeCapabilities) -> Result<Self, CoreError> {
        if network.as_bytes().iter().all(|byte| *byte == 0) {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "NAP admission requires a nonzero network identity",
            ));
        }
        Ok(Self {
            network,
            owner: Arc::new(()),
            ledger: NapAdmissionLedger::new(host),
        })
    }

    pub const fn network(&self) -> NetworkId {
        self.network
    }

    /// Caller must independently verify the candidate's current TLS identity,
    /// durable network approval, and actual PAN attachment before activation.
    pub fn reserve(
        &self,
        authenticated_candidate: NodeId,
    ) -> Result<(NetworkNapTicket, NapAdmissionDecision), CoreError> {
        let (inner, decision) = self.ledger.reserve(authenticated_candidate)?;
        Ok((
            NetworkNapTicket {
                network: self.network,
                owner: Arc::clone(&self.owner),
                inner,
            },
            decision,
        ))
    }

    pub fn activate(&self, ticket: &NetworkNapTicket) -> Result<bool, CoreError> {
        if !self.owns(ticket) {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP ticket belongs to a different network owner",
            ));
        }
        self.ledger.activate(ticket.inner)
    }

    /// Delayed callbacks for a replaced NAP instance are harmless.
    pub fn retire(&self, ticket: &NetworkNapTicket) -> Result<bool, CoreError> {
        if !self.owns(ticket) {
            return Ok(false);
        }
        self.ledger.retire(ticket.inner)
    }

    /// Permanently close this NAP owner and invalidate its tickets. Create a
    /// new scoped owner for a subsequent NAP instance; never reuse this one.
    pub fn close(&self) -> Result<(), CoreError> {
        self.ledger.close()
    }

    pub fn update_capabilities(&self, host: NodeCapabilities) -> Result<(), CoreError> {
        self.ledger.update_capabilities(host)
    }

    pub fn counts(&self) -> Result<(usize, usize), CoreError> {
        self.ledger.counts()
    }

    fn owns(&self, ticket: &NetworkNapTicket) -> bool {
        ticket.network == self.network && Arc::ptr_eq(&ticket.owner, &self.owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilitySource, Sourced};

    fn node(id: u8) -> NodeId {
        NodeId::from_bytes([id; 16])
    }

    fn network(id: u8) -> NetworkId {
        NetworkId::from_bytes([id; 16])
    }

    fn capabilities(ceiling: u16) -> NodeCapabilities {
        NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Discovered)),
            nap: Some(Sourced::new(true, CapabilitySource::Measured)),
            connection_policy_ceiling: Some(Sourced::new(ceiling, CapabilitySource::Configured)),
            ..NodeCapabilities::default()
        }
    }

    #[test]
    fn wrong_network_and_restarted_owner_cannot_use_same_generation() {
        let original = NetworkNapAdmission::new(network(1), capabilities(1)).unwrap();
        let restarted = NetworkNapAdmission::new(network(1), capabilities(1)).unwrap();
        let other = NetworkNapAdmission::new(network(2), capabilities(1)).unwrap();
        let (old, _) = original.reserve(node(3)).unwrap();
        let (new, _) = restarted.reserve(node(3)).unwrap();
        let (foreign, _) = other.reserve(node(3)).unwrap();
        assert_eq!(old.generation(), new.generation());
        assert_eq!(old.candidate(), node(3));
        assert_eq!(old.network(), network(1));
        assert_eq!(
            restarted.activate(&old).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert!(!restarted.retire(&old).unwrap());
        assert_eq!(
            other.activate(&old).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert!(!other.retire(&old).unwrap());
        assert!(!original.retire(&foreign).unwrap());
        assert_eq!(restarted.counts().unwrap(), (0, 1));
        assert!(restarted.activate(&new).unwrap());
        assert!(original.activate(&old).unwrap());
        assert_eq!(other.counts().unwrap(), (0, 1));
    }

    #[test]
    fn local_teardown_and_policy_change_remain_generation_safe() {
        let admission = NetworkNapAdmission::new(network(1), capabilities(2)).unwrap();
        let (first, _) = admission.reserve(node(3)).unwrap();
        admission.update_capabilities(capabilities(1)).unwrap();
        assert!(admission.activate(&first).unwrap());
        assert!(admission.reserve(node(4)).is_err());
        assert!(admission.retire(&first).unwrap());
        assert!(!admission.retire(&first).unwrap());
        let (second, _) = admission.reserve(node(3)).unwrap();
        assert_ne!(first.generation(), second.generation());
        assert!(!admission.retire(&first).unwrap());
        assert!(admission.activate(&second).unwrap());
    }

    #[test]
    fn closed_owner_cannot_reopen_or_retire_replacement_owner() {
        let original = NetworkNapAdmission::new(network(1), capabilities(2)).unwrap();
        let (old, _) = original.reserve(node(3)).unwrap();
        original.close().unwrap();
        assert_eq!(original.counts().unwrap(), (0, 0));
        assert_eq!(original.activate(&old).unwrap_err().kind(), ErrorKind::InvalidState);
        assert!(!original.retire(&old).unwrap());
        assert!(original.reserve(node(4)).is_err());
        assert!(original.update_capabilities(capabilities(2)).is_err());
        let replacement = NetworkNapAdmission::new(network(1), capabilities(2)).unwrap();
        let (fresh, _) = replacement.reserve(node(3)).unwrap();
        assert!(!replacement.retire(&old).unwrap());
        assert!(replacement.activate(&fresh).unwrap());
        assert_eq!(replacement.counts().unwrap(), (1, 0));
    }

    #[test]
    fn zero_network_identity_is_not_an_admission_scope() {
        assert_eq!(
            NetworkNapAdmission::new(network(0), capabilities(1))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
    }
}

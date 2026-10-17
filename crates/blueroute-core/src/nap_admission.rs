//! P6-007: conservative, deterministic multi-client NAP admission policy.
//!
//! This pure policy has no Bluetooth or membership side effects. Callers must
//! independently prove that every observed attachment and candidate identity
//! is authenticated and belongs to the selected network. A capability ceiling
//! is a local policy input, not a measured or universal radio limit.
use std::collections::BTreeSet;

use crate::{CoreError, ErrorKind, NodeCapabilities, NodeId, Sourced};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NapAdmissionDecision {
    pub candidate: NodeId,
    pub already_attached: bool,
    pub connected_before: u16,
    pub remaining_after: u16,
    /// Preserves the original evidence provenance (measured/configured/default).
    pub policy_ceiling: Sourced<u16>,
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidInput, message)
}

fn unavailable(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::CapabilityUnavailable, message)
}

fn valid_id(node: NodeId) -> bool {
    node.as_bytes().iter().any(|byte| *byte != 0)
}

/// Evaluate one candidate against a conservative, explicitly sourced NAP
/// ceiling. A full star allows an idempotent already-attached candidate but
/// never a new one. Duplicated or impossible attachment observations fail
/// closed instead of silently deduplicating or overbooking the radio.
///
/// This function does NOT authenticate the candidate, authorize membership,
/// reserve a concurrent admission slot, or connect a PANU. The actual daemon
/// must serialize concurrent connection attempts and re-observe attachments
/// at the admission boundary.
pub fn plan_nap_client_admission(
    host: &NodeCapabilities,
    active_clients: &[NodeId],
    candidate: NodeId,
) -> Result<NapAdmissionDecision, CoreError> {
    if !valid_id(candidate) {
        return Err(invalid(
            "NAP admission requires a nonzero candidate identity",
        ));
    }
    if host.adapter_usable.as_ref().map(|value| value.value) != Some(true)
        || host.can_host_pan() != Some(true)
    {
        return Err(unavailable(
            "local adapter or NAP capability is unavailable",
        ));
    }
    let policy_ceiling = host
        .connection_policy_ceiling
        .clone()
        .ok_or_else(|| unavailable("NAP client ceiling is unknown"))?;
    if policy_ceiling.value == 0 {
        return Err(invalid("NAP client ceiling must be positive"));
    }

    let mut seen = BTreeSet::new();
    for node in active_clients {
        if !valid_id(*node) || !seen.insert(*node) {
            return Err(invalid("invalid or duplicate active PANU identity"));
        }
    }
    let connected_before = u16::try_from(seen.len())
        .map_err(|_| invalid("observed PANU count exceeds supported range"))?;
    if connected_before > policy_ceiling.value {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "observed PANU count exceeds local policy ceiling",
        ));
    }
    let already_attached = seen.contains(&candidate);
    if !already_attached && connected_before == policy_ceiling.value {
        return Err(unavailable("local NAP client ceiling reached"));
    }
    let connected_after = connected_before + u16::from(!already_attached);
    Ok(NapAdmissionDecision {
        candidate,
        already_attached,
        connected_before,
        remaining_after: policy_ceiling.value - connected_after,
        policy_ceiling,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn two_clients_admit_with_source_and_remaining_capacity() {
        let decision = plan_nap_client_admission(&host(Some(3)), &[peer(2)], peer(3)).unwrap();
        assert_eq!(decision.candidate, peer(3));
        assert_eq!(decision.connected_before, 1);
        assert_eq!(decision.remaining_after, 1);
        assert!(!decision.already_attached);
        assert_eq!(decision.policy_ceiling.source, CapabilitySource::Configured);
        assert_eq!(decision.policy_ceiling.value, 3);
    }

    #[test]
    fn at_capacity_existing_peer_is_idempotent_but_new_peer_is_denied() {
        let active = [peer(2), peer(3)];
        let already = plan_nap_client_admission(&host(Some(2)), &active, peer(2)).unwrap();
        assert!(already.already_attached);
        assert_eq!(already.remaining_after, 0);
        assert_eq!(
            plan_nap_client_admission(&host(Some(2)), &active, peer(4))
                .unwrap_err()
                .kind(),
            ErrorKind::CapabilityUnavailable
        );
    }

    #[test]
    fn missing_or_unusable_nap_capabilities_fail_closed() {
        let mut capabilities = host(None);
        assert_eq!(
            plan_nap_client_admission(&capabilities, &[], peer(2))
                .unwrap_err()
                .kind(),
            ErrorKind::CapabilityUnavailable
        );
        capabilities = host(Some(2));
        capabilities.adapter_usable = None;
        assert!(plan_nap_client_admission(&capabilities, &[], peer(2)).is_err());
        capabilities.adapter_usable = Some(Sourced::new(false, CapabilitySource::Measured));
        assert!(plan_nap_client_admission(&capabilities, &[], peer(2)).is_err());
        capabilities = host(Some(2));
        capabilities.nap = Some(Sourced::new(false, CapabilitySource::Measured));
        assert!(plan_nap_client_admission(&capabilities, &[], peer(2)).is_err());
        capabilities = host(Some(0));
        assert_eq!(
            plan_nap_client_admission(&capabilities, &[], peer(2))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn corrupt_duplicate_and_zero_id_observations_are_rejected() {
        let capabilities = host(Some(3));
        assert!(plan_nap_client_admission(&capabilities, &[], peer(0)).is_err());
        assert!(plan_nap_client_admission(&capabilities, &[peer(2), peer(2)], peer(3)).is_err());
        assert!(plan_nap_client_admission(&capabilities, &[peer(0)], peer(3)).is_err());
        assert_eq!(
            plan_nap_client_admission(&host(Some(1)), &[peer(2), peer(3)], peer(2))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn order_does_not_change_capacity_or_idempotence() {
        let capabilities = host(Some(4));
        let forward =
            plan_nap_client_admission(&capabilities, &[peer(2), peer(3)], peer(3)).unwrap();
        let reversed =
            plan_nap_client_admission(&capabilities, &[peer(3), peer(2)], peer(3)).unwrap();
        assert_eq!(forward, reversed);
        assert_eq!(forward.remaining_after, 2);
    }
}

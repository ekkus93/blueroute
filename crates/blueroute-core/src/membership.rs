use std::collections::BTreeSet;

use crate::{CoreError, DisplayName, ErrorKind, NetworkId, NodeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipState {
    NotMember,
    Joining,
    Member,
    Leaving,
}

impl MembershipState {
    pub fn transition(self, next: Self) -> Result<Self, CoreError> {
        let allowed = matches!(
            (self, next),
            (Self::NotMember, Self::Joining)
                | (Self::Joining, Self::Member)
                | (Self::Joining, Self::NotMember)
                | (Self::Member, Self::Leaving)
                | (Self::Leaving, Self::NotMember)
                | (Self::Leaving, Self::Member)
        );

        if allowed || self == next {
            Ok(next)
        } else {
            Err(CoreError::new(
                ErrorKind::InvalidState,
                format!("invalid membership transition from {self:?} to {next:?}"),
            ))
        }
    }
}

/// Local durable/working membership state for one BlueRoute network.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkMembership {
    pub network_id: NetworkId,
    pub network_name: DisplayName,
    pub state: MembershipState,
    trusted_peers: BTreeSet<NodeId>,
}

impl NetworkMembership {
    pub fn new(network_id: NetworkId, network_name: DisplayName) -> Self {
        Self {
            network_id,
            network_name,
            state: MembershipState::NotMember,
            trusted_peers: BTreeSet::new(),
        }
    }

    pub fn trust_peer(&mut self, peer: NodeId) -> bool {
        self.trusted_peers.insert(peer)
    }

    pub fn forget_peer(&mut self, peer: &NodeId) -> bool {
        self.trusted_peers.remove(peer)
    }

    pub fn is_peer_trusted(&self, peer: &NodeId) -> bool {
        self.trusted_peers.contains(peer)
    }

    pub fn trusted_peers(&self) -> impl Iterator<Item = &NodeId> {
        self.trusted_peers.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network_id() -> NetworkId {
        NetworkId::from_bytes([1; 16])
    }

    #[test]
    fn legal_join_and_leave_transitions_work() {
        let state = MembershipState::NotMember
            .transition(MembershipState::Joining)
            .unwrap()
            .transition(MembershipState::Member)
            .unwrap()
            .transition(MembershipState::Leaving)
            .unwrap()
            .transition(MembershipState::NotMember)
            .unwrap();
        assert_eq!(state, MembershipState::NotMember);
    }

    #[test]
    fn illegal_transition_is_rejected() {
        assert!(
            MembershipState::NotMember
                .transition(MembershipState::Member)
                .is_err()
        );
    }

    #[test]
    fn network_name_is_separate_from_network_identity() {
        let mut membership =
            NetworkMembership::new(network_id(), DisplayName::new("Workshop").unwrap());
        membership.network_name = DisplayName::new("Renamed workshop").unwrap();
        assert_eq!(membership.network_id, network_id());
    }

    #[test]
    fn trusted_peers_are_deterministic_and_revocable() {
        let mut membership =
            NetworkMembership::new(network_id(), DisplayName::new("Workshop").unwrap());
        let high = NodeId::from_bytes([9; 16]);
        let low = NodeId::from_bytes([2; 16]);
        membership.trust_peer(high);
        membership.trust_peer(low);
        let peers: Vec<_> = membership.trusted_peers().copied().collect();
        assert_eq!(peers, vec![low, high]);
        assert!(membership.forget_peer(&low));
        assert!(!membership.is_peer_trusted(&low));
    }
}

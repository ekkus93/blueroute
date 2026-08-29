use std::collections::{BTreeMap, btree_map::Entry};

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

/// Durable membership/trust facts remembered for one peer in a BlueRoute network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerMembership {
    pub node_id: NodeId,
    member: bool,
    trusted: bool,
}

impl PeerMembership {
    pub const fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            member: false,
            trusted: false,
        }
    }

    pub const fn is_member(&self) -> bool {
        self.member
    }

    pub const fn is_trusted(&self) -> bool {
        self.trusted
    }
}

/// Local durable/working membership state for one BlueRoute network.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkMembership {
    pub network_id: NetworkId,
    pub network_name: DisplayName,
    pub state: MembershipState,
    peers: BTreeMap<NodeId, PeerMembership>,
}

impl NetworkMembership {
    pub fn new(network_id: NetworkId, network_name: DisplayName) -> Self {
        Self {
            network_id,
            network_name,
            state: MembershipState::NotMember,
            peers: BTreeMap::new(),
        }
    }

    pub fn remember_peer(&mut self, peer: NodeId) -> bool {
        match self.peers.entry(peer) {
            Entry::Vacant(entry) => {
                entry.insert(PeerMembership::new(peer));
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub fn set_peer_member(&mut self, peer: NodeId, member: bool) -> bool {
        let peer = self.peers.entry(peer).or_insert(PeerMembership::new(peer));
        let changed = peer.member != member;
        peer.member = member;
        changed
    }

    pub fn trust_peer(&mut self, peer: NodeId) -> bool {
        let peer = self.peers.entry(peer).or_insert(PeerMembership::new(peer));
        let changed = !peer.trusted;
        peer.trusted = true;
        changed
    }

    pub fn untrust_peer(&mut self, peer: &NodeId) -> bool {
        let Some(peer) = self.peers.get_mut(peer) else {
            return false;
        };
        let changed = peer.trusted;
        peer.trusted = false;
        changed
    }

    pub fn forget_peer(&mut self, peer: &NodeId) -> bool {
        self.peers.remove(peer).is_some()
    }

    pub fn is_peer_known(&self, peer: &NodeId) -> bool {
        self.peers.contains_key(peer)
    }

    pub fn is_peer_member(&self, peer: &NodeId) -> bool {
        self.peers.get(peer).is_some_and(PeerMembership::is_member)
    }

    pub fn is_peer_trusted(&self, peer: &NodeId) -> bool {
        self.peers.get(peer).is_some_and(PeerMembership::is_trusted)
    }

    pub fn peers(&self) -> impl Iterator<Item = &PeerMembership> {
        self.peers.values()
    }

    pub fn trusted_peers(&self) -> impl Iterator<Item = &NodeId> {
        self.peers
            .iter()
            .filter_map(|(node_id, peer)| peer.trusted.then_some(node_id))
    }
}

/// Deterministic collection of remembered BlueRoute networks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MembershipRegistry {
    networks: BTreeMap<NetworkId, NetworkMembership>,
}

impl MembershipRegistry {
    pub fn remember_network(&mut self, membership: NetworkMembership) -> Option<NetworkMembership> {
        self.networks.insert(membership.network_id, membership)
    }

    pub fn network(&self, network_id: &NetworkId) -> Option<&NetworkMembership> {
        self.networks.get(network_id)
    }

    pub fn network_mut(&mut self, network_id: &NetworkId) -> Option<&mut NetworkMembership> {
        self.networks.get_mut(network_id)
    }

    pub fn forget_network(&mut self, network_id: &NetworkId) -> Option<NetworkMembership> {
        self.networks.remove(network_id)
    }

    pub fn networks(&self) -> impl Iterator<Item = &NetworkMembership> {
        self.networks.values()
    }

    pub fn len(&self) -> usize {
        self.networks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
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
    fn known_membership_and_trust_are_independent_and_revocable() {
        let mut membership =
            NetworkMembership::new(network_id(), DisplayName::new("Workshop").unwrap());
        let peer = NodeId::from_bytes([2; 16]);

        assert!(membership.remember_peer(peer));
        assert!(membership.is_peer_known(&peer));
        assert!(!membership.is_peer_member(&peer));
        assert!(!membership.is_peer_trusted(&peer));

        assert!(membership.set_peer_member(peer, true));
        assert!(membership.is_peer_member(&peer));
        assert!(!membership.is_peer_trusted(&peer));

        assert!(membership.trust_peer(peer));
        assert!(membership.is_peer_trusted(&peer));
        assert!(membership.untrust_peer(&peer));
        assert!(!membership.is_peer_trusted(&peer));
        assert!(membership.is_peer_member(&peer));

        assert!(membership.forget_peer(&peer));
        assert!(!membership.is_peer_known(&peer));
    }

    #[test]
    fn trusted_peers_are_deterministic() {
        let mut membership =
            NetworkMembership::new(network_id(), DisplayName::new("Workshop").unwrap());
        let high = NodeId::from_bytes([9; 16]);
        let low = NodeId::from_bytes([2; 16]);
        membership.trust_peer(high);
        membership.trust_peer(low);
        let peers: Vec<_> = membership.trusted_peers().copied().collect();
        assert_eq!(peers, vec![low, high]);
    }

    #[test]
    fn registry_remembers_and_forgets_networks_deterministically() {
        let low = NetworkId::from_bytes([1; 16]);
        let high = NetworkId::from_bytes([9; 16]);
        let mut registry = MembershipRegistry::default();
        registry.remember_network(NetworkMembership::new(
            high,
            DisplayName::new("High").unwrap(),
        ));
        registry.remember_network(NetworkMembership::new(
            low,
            DisplayName::new("Low").unwrap(),
        ));

        let ids: Vec<_> = registry
            .networks()
            .map(|membership| membership.network_id)
            .collect();
        assert_eq!(ids, vec![low, high]);
        assert!(registry.forget_network(&low).is_some());
        assert!(registry.network(&low).is_none());
    }
}

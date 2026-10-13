//! Untrusted candidate observations are separate from active PAN links.
//! Device discovery and peer capability claims must never authorize routing.

use std::collections::{BTreeMap, BTreeSet};

use crate::{CoreError, ErrorKind, NodeCapabilities, NodeId, TopologyGraph};

pub const CANDIDATE_LIFETIME_MS: u64 = 120_000;
pub const MAX_CANDIDATES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateSource {
    BluetoothDiscovery,
    PeerAdvertisement,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateObservation {
    pub node: NodeId,
    pub source: CandidateSource,
    pub capabilities: NodeCapabilities,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct NeighborObservations {
    candidates: BTreeMap<NodeId, CandidateObservation>,
}

impl NeighborObservations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// The caller supplies a locally observed monotonic timestamp, never one
    /// asserted by an untrusted remote device.
    pub fn observe(
        &mut self,
        local: NodeId,
        peer: NodeId,
        source: CandidateSource,
        capabilities: NodeCapabilities,
        observed_at_ms: u64,
    ) -> Result<bool, CoreError> {
        if local == peer {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "self-discovery candidate is invalid",
            ));
        }
        let expires_at_ms = observed_at_ms.checked_add(CANDIDATE_LIFETIME_MS).ok_or_else(|| {
            CoreError::new(ErrorKind::InvalidInput, "candidate timestamp overflow")
        })?;
        if let Some(previous) = self.candidates.get(&peer) {
            if observed_at_ms <= previous.observed_at_ms {
                return Ok(false);
            }
        } else if self.candidates.len() >= MAX_CANDIDATES {
            return Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "candidate discovery table is full",
            ));
        }
        self.candidates.insert(
            peer,
            CandidateObservation {
                node: peer,
                source,
                capabilities,
                observed_at_ms,
                expires_at_ms,
            },
        );
        Ok(true)
    }

    pub fn candidates_at(&self, now_ms: u64) -> impl Iterator<Item = &CandidateObservation> + '_ {
        self.candidates
            .values()
            .filter(move |candidate| now_ms < candidate.expires_at_ms)
    }

    pub fn expire(&mut self, now_ms: u64) -> usize {
        let before = self.candidates.len();
        self.candidates
            .retain(|_, candidate| now_ms < candidate.expires_at_ms);
        before - self.candidates.len()
    }

    /// Only independently established, active and healthy PAN links in the
    /// topology graph count as direct neighbors.
    pub fn active_direct_peers(&self, graph: &TopologyGraph, local: NodeId) -> BTreeSet<NodeId> {
        graph.direct_neighbors(local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilitySource, LinkHealth, LinkId, LinkState, PanLink, PanSegment, SegmentId, Sourced};

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn capabilities() -> NodeCapabilities {
        NodeCapabilities {
            nap: Some(Sourced::new(true, CapabilitySource::Discovered)),
            ..NodeCapabilities::default()
        }
    }

    #[test]
    fn discovery_never_implies_a_working_pan_link() {
        let mut observed = NeighborObservations::new();
        let mut graph = TopologyGraph::default();
        graph.add_node(node(1), NodeCapabilities::default());
        graph.add_node(node(2), NodeCapabilities::default());
        observed.observe(node(1), node(2), CandidateSource::BluetoothDiscovery, capabilities(), 10).unwrap();
        assert_eq!(observed.candidates_at(10).count(), 1);
        assert!(observed.active_direct_peers(&graph, node(1)).is_empty());

        let segment = SegmentId::from_bytes([9; 16]);
        graph.add_segment(PanSegment::new(segment, node(1))).unwrap();
        graph.add_link(PanLink {
            id: LinkId::from_bytes([7; 16]),
            segment_id: segment,
            nap: node(1),
            panu: node(2),
            state: LinkState::Connecting,
            health: LinkHealth::Unknown,
            cost: 1,
        }).unwrap();
        assert!(observed.active_direct_peers(&graph, node(1)).is_empty());
        graph.remove_link(&LinkId::from_bytes([7; 16]));
        graph.add_link(PanLink {
            id: LinkId::from_bytes([8; 16]),
            segment_id: segment,
            nap: node(1),
            panu: node(2),
            state: LinkState::Active,
            health: LinkHealth::Healthy,
            cost: 1,
        }).unwrap();
        assert_eq!(observed.active_direct_peers(&graph, node(1)), BTreeSet::from([node(2)]));
        observed.expire(10 + CANDIDATE_LIFETIME_MS);
        assert_eq!(observed.active_direct_peers(&graph, node(1)), BTreeSet::from([node(2)]));
    }

    #[test]
    fn expired_and_out_of_order_candidates_are_ignored() {
        let mut observed = NeighborObservations::new();
        assert!(observed.observe(node(1), node(2), CandidateSource::BluetoothDiscovery, capabilities(), 100).unwrap());
        assert!(!observed.observe(node(1), node(2), CandidateSource::PeerAdvertisement, NodeCapabilities::default(), 99).unwrap());
        assert_eq!(observed.candidates_at(100).next().unwrap().source, CandidateSource::BluetoothDiscovery);
        assert_eq!(observed.candidates_at(100 + CANDIDATE_LIFETIME_MS).count(), 0);
        assert_eq!(observed.expire(100 + CANDIDATE_LIFETIME_MS), 1);
        assert!(observed.is_empty());
    }

    #[test]
    fn candidate_ids_timestamps_and_retained_count_are_bounded() {
        let mut observed = NeighborObservations::new();
        assert!(observed.observe(node(1), node(1), CandidateSource::BluetoothDiscovery, capabilities(), 1).is_err());
        assert!(observed.observe(node(1), node(2), CandidateSource::BluetoothDiscovery, capabilities(), u64::MAX).is_err());
        for i in 0..MAX_CANDIDATES {
            let mut bytes = [0; 16];
            bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
            observed.observe(node(1), NodeId::from_bytes(bytes), CandidateSource::BluetoothDiscovery, capabilities(), 10).unwrap();
        }
        assert_eq!(observed.len(), MAX_CANDIDATES);
        assert!(observed.observe(node(1), node(2), CandidateSource::BluetoothDiscovery, capabilities(), 10).is_err());
        assert_eq!(observed.expire(10 + CANDIDATE_LIFETIME_MS), MAX_CANDIDATES);
    }
}

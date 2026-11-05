//! Neighbor observation model (P8-001).
//!
//! Keeps what a node has *seen* (candidate reachability from discovery) strictly separate from
//! what it is *using* (active PAN links). Topology and routing consume `active_links()`;
//! discovery and policy consume `candidates()`. Time is an injected monotonic `Duration`.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::{LinkId, NodeCapabilities, NodeId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NeighborLink {
    /// Seen or advertised, but no PAN link is in use. Expires without refresh.
    Candidate,
    /// A direct PAN link is in use. Lives as long as the link does.
    Active(LinkId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Neighbor {
    pub node: NodeId,
    pub link: NeighborLink,
    pub first_seen: Duration,
    pub last_seen: Duration,
    /// Candidates are dropped at this time unless refreshed. Unused for active links.
    pub expires_at: Duration,
    pub capabilities: NodeCapabilities,
}

impl Neighbor {
    pub fn is_active(&self) -> bool {
        matches!(self.link, NeighborLink::Active(_))
    }
}

#[derive(Clone, Debug)]
pub struct NeighborTable {
    ttl: Duration,
    entries: BTreeMap<NodeId, Neighbor>,
}

impl NeighborTable {
    pub fn new(candidate_ttl: Duration) -> Self {
        Self {
            ttl: candidate_ttl,
            entries: BTreeMap::new(),
        }
    }

    /// Record a sighting. Refreshes a candidate; for an active neighbor it only updates
    /// `last_seen` and capabilities, never demoting the link.
    pub fn observe(&mut self, node: NodeId, capabilities: NodeCapabilities, now: Duration) {
        let ttl = self.ttl;
        self.entries
            .entry(node)
            .and_modify(|n| {
                n.last_seen = now;
                n.capabilities = capabilities.clone();
                if !n.is_active() {
                    n.expires_at = now + ttl;
                }
            })
            .or_insert(Neighbor {
                node,
                link: NeighborLink::Candidate,
                first_seen: now,
                last_seen: now,
                expires_at: now + ttl,
                capabilities,
            });
    }

    /// A direct PAN link to `node` became active.
    pub fn mark_active(&mut self, node: NodeId, link: LinkId, now: Duration) {
        let ttl = self.ttl;
        let entry = self.entries.entry(node).or_insert(Neighbor {
            node,
            link: NeighborLink::Candidate,
            first_seen: now,
            last_seen: now,
            expires_at: now + ttl,
            capabilities: NodeCapabilities::default(),
        });
        entry.link = NeighborLink::Active(link);
        entry.last_seen = now;
    }

    /// The link to `node` ended. It stays a candidate for one TTL, since it was just seen.
    pub fn mark_inactive(&mut self, node: NodeId, now: Duration) {
        let ttl = self.ttl;
        if let Some(entry) = self.entries.get_mut(&node)
            && entry.is_active()
        {
            entry.link = NeighborLink::Candidate;
            entry.last_seen = now;
            entry.expires_at = now + ttl;
        }
    }

    /// Drop expired candidates and return them. Active links are never expired by time.
    pub fn expire(&mut self, now: Duration) -> Vec<NodeId> {
        let expired: Vec<NodeId> = self
            .entries
            .values()
            .filter(|n| !n.is_active() && n.expires_at <= now)
            .map(|n| n.node)
            .collect();
        for node in &expired {
            self.entries.remove(node);
        }
        expired
    }

    pub fn get(&self, node: &NodeId) -> Option<&Neighbor> {
        self.entries.get(node)
    }

    /// Neighbors seen but not linked (physical/candidate view).
    pub fn candidates(&self) -> impl Iterator<Item = &Neighbor> {
        self.entries.values().filter(|n| !n.is_active())
    }

    /// Neighbors with a direct PAN link in use (input to the active routed graph).
    pub fn active_links(&self) -> impl Iterator<Item = (&Neighbor, LinkId)> {
        self.entries.values().filter_map(|n| match &n.link {
            NeighborLink::Active(id) => Some((n, *id)),
            NeighborLink::Candidate => None,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilitySource, Sourced};

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    fn table() -> NeighborTable {
        NeighborTable::new(s(30))
    }

    #[test]
    fn candidate_and_active_views_are_disjoint() {
        let mut t = table();
        t.observe(n(1), NodeCapabilities::default(), s(0));
        t.observe(n(2), NodeCapabilities::default(), s(0));
        t.mark_active(n(2), LinkId::from_bytes([9; 16]), s(1));
        assert_eq!(
            t.candidates().map(|x| x.node).collect::<Vec<_>>(),
            vec![n(1)]
        );
        let active: Vec<_> = t.active_links().map(|(x, id)| (x.node, id)).collect();
        assert_eq!(active, vec![(n(2), LinkId::from_bytes([9; 16]))]);
    }

    #[test]
    fn candidates_expire_unless_refreshed_and_active_links_do_not() {
        let mut t = table();
        t.observe(n(1), NodeCapabilities::default(), s(0));
        t.observe(n(2), NodeCapabilities::default(), s(0));
        t.mark_active(n(3), LinkId::from_bytes([1; 16]), s(0));
        t.observe(n(2), NodeCapabilities::default(), s(20));
        assert_eq!(t.expire(s(30)), vec![n(1)]);
        assert!(t.get(&n(2)).is_some(), "refresh extended expiry");
        assert_eq!(t.expire(s(50)), vec![n(2)]);
        assert!(t.get(&n(3)).is_some(), "active links never time out");
    }

    #[test]
    fn observing_an_active_neighbor_never_demotes_the_link() {
        let mut t = table();
        t.mark_active(n(1), LinkId::from_bytes([1; 16]), s(0));
        t.observe(n(1), NodeCapabilities::default(), s(5));
        assert!(t.get(&n(1)).unwrap().is_active());
        assert_eq!(t.get(&n(1)).unwrap().last_seen, s(5));
        assert_eq!(t.get(&n(1)).unwrap().first_seen, s(0));
    }

    #[test]
    fn link_loss_returns_to_a_fresh_candidate_that_then_expires() {
        let mut t = table();
        t.mark_active(n(1), LinkId::from_bytes([1; 16]), s(0));
        t.mark_inactive(n(1), s(100));
        assert!(!t.get(&n(1)).unwrap().is_active());
        assert!(t.expire(s(120)).is_empty());
        assert_eq!(t.expire(s(130)), vec![n(1)]);
        assert!(t.is_empty());
    }

    #[test]
    fn capability_annotations_follow_the_latest_observation() {
        let mut t = table();
        let caps = |nap| NodeCapabilities {
            nap: Some(Sourced::new(nap, CapabilitySource::Discovered)),
            ..NodeCapabilities::default()
        };
        t.observe(n(1), caps(false), s(0));
        t.observe(n(1), caps(true), s(1));
        assert_eq!(
            t.get(&n(1)).unwrap().capabilities.can_host_pan(),
            Some(true)
        );
    }
}

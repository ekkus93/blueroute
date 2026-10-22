//! P9-005 observation-driven, debounced NAP hub-loss detection.
//!
//! This is a software-only gate over independently sampled *authoritative*
//! local PAN observations. It does not discover radio loss, authenticate a
//! remote claim, or authorize reconnects. Callers must serialize samples per
//! network and forget hubs before intentional teardown.
use std::collections::{BTreeMap, BTreeSet};

use crate::{CoreError, ErrorKind, NapLink, NodeId};

/// Tracks hubs that were actually observed, rather than inferring loss from
/// an untrusted discovery advertisement or an absent initial snapshot.
#[derive(Clone, Debug)]
pub struct HubLossTracker {
    min_absence_ms: u64,
    last_sample_ms: Option<u64>,
    seen_hubs: BTreeSet<NodeId>,
    missing_since: BTreeMap<NodeId, u64>,
    lost_hubs: BTreeSet<NodeId>,
}

impl HubLossTracker {
    pub fn new(min_absence_ms: u64) -> Result<Self, CoreError> {
        if min_absence_ms == 0 {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "hub-loss confirmation interval must be positive",
            ));
        }
        Ok(Self {
            min_absence_ms,
            last_sample_ms: None,
            seen_hubs: BTreeSet::new(),
            missing_since: BTreeMap::new(),
            lost_hubs: BTreeSet::new(),
        })
    }

    /// Consume a fresh snapshot of *observed*, not requested, PAN links.
    ///
    /// A hub must first be positively observed, then be entirely absent from
    /// at least two strictly time-ordered samples separated by the configured
    /// interval. Partial loss of a hub's clients does not imply hub loss.
    /// Reappearance cancels a pending/confirmed loss. Invalid observations
    /// and clock regressions leave tracker state unchanged.
    ///
    /// The result is the set of *currently* confirmed lost hubs, not just new
    /// notifications. Callers must still check the latest PAN snapshot before
    /// planning recovery and must not equate these observations with trust.
    pub fn observe(
        &mut self,
        now_ms: u64,
        observed_links: &BTreeSet<NapLink>,
    ) -> Result<BTreeSet<NodeId>, CoreError> {
        if self
            .last_sample_ms
            .is_some_and(|previous| now_ms <= previous)
        {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "stale or regressed hub-loss observation timestamp",
            ));
        }
        let mut active = BTreeSet::new();
        let mut clients = BTreeSet::new();
        let zero = NodeId::from_bytes([0; 16]);
        for link in observed_links {
            if link.hub == zero
                || link.client == zero
                || link.hub == link.client
                || !clients.insert(link.client)
            {
                return Err(CoreError::new(
                    ErrorKind::InvalidInput,
                    "invalid or conflicting observed PAN links",
                ));
            }
            active.insert(link.hub);
        }

        self.last_sample_ms = Some(now_ms);
        for hub in &active {
            self.seen_hubs.insert(*hub);
            self.missing_since.remove(hub);
            self.lost_hubs.remove(hub);
        }
        // A previously observed NAP can legitimately become a PANU client
        // during an independently observed role change. The node is still
        // reachable, so it must not be declared lost or excluded from
        // recovery planning. Re-arm hub-loss detection if it hosts again.
        let demoted: Vec<_> = self
            .seen_hubs
            .difference(&active)
            .filter(|hub| clients.contains(hub))
            .copied()
            .collect();
        for hub in demoted {
            self.forget_hub(hub);
        }
        for hub in self.seen_hubs.difference(&active) {
            if self.lost_hubs.contains(hub) {
                continue;
            }
            match self.missing_since.get(hub).copied() {
                Some(since) if now_ms - since >= self.min_absence_ms => {
                    self.lost_hubs.insert(*hub);
                    self.missing_since.remove(hub);
                }
                Some(_) => {}
                None => {
                    self.missing_since.insert(*hub, now_ms);
                }
            }
        }
        Ok(self.lost_hubs.clone())
    }

    /// Stop tracking an intentionally removed hub; this must be called before
    /// the corresponding deliberate disconnect becomes observable.
    pub fn forget_hub(&mut self, hub: NodeId) {
        self.seen_hubs.remove(&hub);
        self.missing_since.remove(&hub);
        self.lost_hubs.remove(&hub);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> NodeId {
        NodeId::from_bytes([n; 16])
    }

    fn link(hub: u8, client: u8) -> NapLink {
        NapLink {
            hub: id(hub),
            client: id(client),
        }
    }

    #[test]
    fn requires_positive_baseline_and_two_separated_absent_samples() {
        let mut tracker = HubLossTracker::new(100).unwrap();
        let empty = BTreeSet::new();
        assert!(tracker.observe(0, &empty).unwrap().is_empty());
        assert!(
            tracker
                .observe(1, &BTreeSet::from([link(1, 3)]))
                .unwrap()
                .is_empty()
        );
        assert!(tracker.observe(2, &empty).unwrap().is_empty());
        assert!(tracker.observe(101, &empty).unwrap().is_empty());
        assert_eq!(
            tracker.observe(102, &empty).unwrap(),
            BTreeSet::from([id(1)])
        );
        assert_eq!(
            tracker.observe(103, &empty).unwrap(),
            BTreeSet::from([id(1)])
        );
    }

    #[test]
    fn brief_gap_and_partial_client_loss_do_not_report_hub_failure() {
        let mut tracker = HubLossTracker::new(10).unwrap();
        let full = BTreeSet::from([link(1, 3), link(1, 4)]);
        assert!(tracker.observe(0, &full).unwrap().is_empty());
        assert!(tracker.observe(1, &BTreeSet::new()).unwrap().is_empty());
        assert!(
            tracker
                .observe(2, &BTreeSet::from([link(1, 4)]))
                .unwrap()
                .is_empty()
        );
        assert!(
            tracker
                .observe(100, &BTreeSet::from([link(1, 4)]))
                .unwrap()
                .is_empty()
        );
        assert!(tracker.observe(101, &BTreeSet::new()).unwrap().is_empty());
        assert!(tracker.observe(110, &BTreeSet::new()).unwrap().is_empty());
        assert_eq!(
            tracker.observe(111, &BTreeSet::new()).unwrap(),
            BTreeSet::from([id(1)])
        );
        assert!(tracker.observe(112, &full).unwrap().is_empty());
        assert!(tracker.observe(113, &BTreeSet::new()).unwrap().is_empty());
    }

    #[test]
    fn bad_observations_and_stale_clocks_cannot_advance_loss() {
        let mut tracker = HubLossTracker::new(10).unwrap();
        tracker.observe(100, &BTreeSet::from([link(1, 3)])).unwrap();
        assert_eq!(
            tracker.observe(100, &BTreeSet::new()).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(
            tracker.observe(99, &BTreeSet::new()).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert!(tracker.observe(101, &BTreeSet::new()).unwrap().is_empty());
        assert!(
            tracker
                .observe(102, &BTreeSet::from([link(1, 3), link(2, 3)]))
                .is_err()
        );
        assert!(tracker.observe(103, &BTreeSet::from([link(0, 3)])).is_err());
        assert!(tracker.observe(104, &BTreeSet::from([link(3, 3)])).is_err());
        assert_eq!(
            tracker.observe(111, &BTreeSet::new()).unwrap(),
            BTreeSet::from([id(1)])
        );
    }

    #[test]
    fn unrelated_hub_remains_healthy_and_deliberate_removal_is_not_loss() {
        let mut tracker = HubLossTracker::new(5).unwrap();
        tracker
            .observe(1, &BTreeSet::from([link(1, 3), link(2, 4)]))
            .unwrap();
        tracker.forget_hub(id(2));
        assert!(tracker.observe(2, &BTreeSet::new()).unwrap().is_empty());
        assert_eq!(
            tracker.observe(7, &BTreeSet::new()).unwrap(),
            BTreeSet::from([id(1)])
        );
        assert!(
            tracker
                .observe(8, &BTreeSet::from([link(1, 3)]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn observed_hub_to_client_role_change_does_not_invent_node_loss() {
        let mut tracker = HubLossTracker::new(5).unwrap();
        let hosting = BTreeSet::from([link(1, 3), link(2, 4)]);
        let demoted = BTreeSet::from([link(2, 1), link(2, 4)]);
        let surviving = BTreeSet::from([link(2, 4)]);

        assert!(tracker.observe(0, &hosting).unwrap().is_empty());
        assert!(tracker.observe(1, &demoted).unwrap().is_empty());
        assert!(tracker.observe(7, &demoted).unwrap().is_empty());
        // A demoted hub is no longer tracked as failed, even after it
        // disappears as a client; a new positive hub observation re-arms it.
        assert!(tracker.observe(8, &surviving).unwrap().is_empty());
        assert!(tracker.observe(9, &hosting).unwrap().is_empty());
        assert!(tracker.observe(10, &surviving).unwrap().is_empty());
        assert_eq!(
            tracker.observe(15, &surviving).unwrap(),
            BTreeSet::from([id(1)])
        );

        // Even a confirmed lost hub is no longer absent once it is
        // independently observed as a PANU client on another hub.
        assert!(tracker.observe(16, &demoted).unwrap().is_empty());
        assert!(tracker.observe(17, &surviving).unwrap().is_empty());
    }

    #[test]
    fn zero_confirmation_interval_is_rejected() {
        assert_eq!(
            HubLossTracker::new(0).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }
}

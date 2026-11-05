//! Topology policy inputs (P9-001).
//!
//! Everything automatic topology planning may look at, as plain data. There is deliberately no
//! field for a computer, adapter, or vendor model: behavior follows capabilities with
//! provenance, per-node capacity, direct-neighbor availability, and link history only.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::{LinkQuality, NodeCapabilities, NodeId, PowerState, TopologyPolicy};

/// Capacity assumed for a node that reports no connection ceiling. Deliberately minimal: the
/// project never generalizes one adapter's measured limit to another.
pub const DEFAULT_UNKNOWN_CAPACITY: u16 = 1;
/// Path cost assumed for a direct PAN hop with no better information.
pub const DEFAULT_LINK_COST: u32 = 10;

/// Observed history of a candidate link, used for stability decisions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinkStability {
    pub recent_failures: u32,
    /// How long the link has been continuously up; zero if not currently active.
    pub uptime: Duration,
}

/// A direct neighbor relationship that could become a PAN link. Endpoints are stored in
/// canonical order so `(a, b)` and `(b, a)` are the same candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateLink {
    pub a: NodeId,
    pub b: NodeId,
    pub quality: Option<LinkQuality>,
    pub stability: LinkStability,
    /// A link that has worked before and should be preferred when otherwise comparable.
    pub known_good: bool,
}

impl CandidateLink {
    pub fn new(x: NodeId, y: NodeId) -> Self {
        let (a, b) = if x <= y { (x, y) } else { (y, x) };
        Self {
            a,
            b,
            quality: None,
            stability: LinkStability::default(),
            known_good: false,
        }
    }

    pub fn connects(&self, node: NodeId) -> Option<NodeId> {
        if node == self.a {
            Some(self.b)
        } else if node == self.b {
            Some(self.a)
        } else {
            None
        }
    }

    /// Lower is better. Combines hop cost, reported quality, recent failures, and history.
    pub fn cost(&self) -> u32 {
        let quality_penalty = self
            .quality
            .map_or(0, |q| u32::from(100_u8.saturating_sub(q.score)) / 5);
        let failure_penalty = self.stability.recent_failures.saturating_mul(20);
        let known_good_bonus = if self.known_good { 5 } else { 0 };
        DEFAULT_LINK_COST
            .saturating_add(quality_penalty)
            .saturating_add(failure_penalty)
            .saturating_sub(known_good_bonus)
    }
}

/// All inputs to one planning run.
#[derive(Clone, Debug, Default)]
pub struct TopologyInputs {
    pub nodes: BTreeMap<NodeId, NodeCapabilities>,
    /// Direct-neighbor availability: which pairs could be linked right now.
    pub candidates: Vec<CandidateLink>,
    pub policy: TopologyPolicy,
    /// Reserved for the later Internet-gateway feature; only a mild tie-breaker today.
    pub gateway_preference: Option<NodeId>,
}

impl TopologyInputs {
    /// A node may act as NAP only when it positively reports that it can.
    pub fn can_host(&self, node: NodeId) -> bool {
        self.nodes
            .get(&node)
            .is_some_and(|caps| caps.can_host_pan() == Some(true))
    }

    /// A node may attach as PANU only when it positively reports that it can.
    pub fn can_attach(&self, node: NodeId) -> bool {
        self.nodes
            .get(&node)
            .is_some_and(|caps| caps.can_join_pan() == Some(true))
    }

    /// Maximum simultaneous PAN links the planner may assign to `node`.
    pub fn capacity(&self, node: NodeId) -> u16 {
        let reported = self
            .nodes
            .get(&node)
            .and_then(|caps| caps.connection_policy_ceiling.as_ref())
            .map_or(DEFAULT_UNKNOWN_CAPACITY, |c| c.value);
        match self.policy.max_active_links {
            Some(limit) => reported.min(limit),
            None => reported,
        }
    }

    /// Higher is a better hub. Uses only capabilities and policy.
    pub fn hub_score(&self, node: NodeId) -> i64 {
        let Some(caps) = self.nodes.get(&node) else {
            return i64::MIN;
        };
        let mut score = i64::from(self.capacity(node)) * 5;
        if let Some(power) = &caps.power_state {
            score += match power.value {
                PowerState::AcPowered if self.policy.prefer_ac_powered_hubs => 50,
                PowerState::AcPowered => 0,
                PowerState::Battery { percent } => i64::from(percent.min(100)) / 4 - 25,
            };
        }
        if let Some(quality) = &caps.link_quality {
            score += i64::from(quality.value.score) / 10;
        }
        if self.gateway_preference == Some(node) {
            score += 10;
        }
        score
    }

    /// Candidate links that touch known nodes only (stale neighbors are ignored).
    pub fn usable_candidates(&self) -> impl Iterator<Item = &CandidateLink> {
        self.candidates.iter().filter(|c| {
            self.nodes.contains_key(&c.a) && self.nodes.contains_key(&c.b) && c.a != c.b
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilitySource, Sourced};

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn src<T>(v: T) -> Sourced<T> {
        Sourced::new(v, CapabilitySource::Configured)
    }

    fn caps(nap: bool, ceiling: Option<u16>, power: Option<PowerState>) -> NodeCapabilities {
        NodeCapabilities {
            panu: Some(src(true)),
            nap: Some(src(nap)),
            connection_policy_ceiling: ceiling.map(src),
            power_state: power.map(src),
            ..NodeCapabilities::default()
        }
    }

    fn inputs(nodes: Vec<(u8, NodeCapabilities)>) -> TopologyInputs {
        TopologyInputs {
            nodes: nodes.into_iter().map(|(i, c)| (n(i), c)).collect(),
            ..TopologyInputs::default()
        }
    }

    #[test]
    fn capacity_uses_reported_ceiling_then_policy_limit_then_conservative_default() {
        let mut i = inputs(vec![
            (1, caps(true, Some(7), None)),
            (2, caps(true, None, None)),
        ]);
        assert_eq!(i.capacity(n(1)), 7);
        assert_eq!(i.capacity(n(2)), DEFAULT_UNKNOWN_CAPACITY);
        assert_eq!(i.capacity(n(9)), DEFAULT_UNKNOWN_CAPACITY);
        i.policy.max_active_links = Some(3);
        assert_eq!(i.capacity(n(1)), 3);
        assert_eq!(i.capacity(n(2)), 1, "a policy limit never raises capacity");
    }

    #[test]
    fn unknown_capabilities_are_not_treated_as_true() {
        let i = inputs(vec![(1, NodeCapabilities::default())]);
        assert!(!i.can_host(n(1)));
        assert!(!i.can_attach(n(1)));
        assert!(!i.can_host(n(9)));
    }

    #[test]
    fn hub_score_follows_power_policy_and_capacity() {
        let ac = caps(true, Some(4), Some(PowerState::AcPowered));
        let low = caps(true, Some(4), Some(PowerState::Battery { percent: 10 }));
        let high = caps(true, Some(4), Some(PowerState::Battery { percent: 90 }));
        let mut i = inputs(vec![(1, ac), (2, low), (3, high)]);
        assert!(i.hub_score(n(1)) > i.hub_score(n(3)));
        assert!(i.hub_score(n(3)) > i.hub_score(n(2)));
        i.policy.prefer_ac_powered_hubs = false;
        assert!(i.hub_score(n(1)) < i.hub_score(n(3)) + 30);
        assert_eq!(i.hub_score(n(9)), i64::MIN);
    }

    #[test]
    fn identical_capabilities_score_identically_whatever_the_node() {
        let c = caps(true, Some(4), Some(PowerState::AcPowered));
        let i = inputs(vec![(1, c.clone()), (200, c)]);
        assert_eq!(i.hub_score(n(1)), i.hub_score(n(200)));
    }

    #[test]
    fn gateway_preference_is_a_mild_tie_breaker() {
        let c = caps(true, Some(4), None);
        let mut i = inputs(vec![(1, c.clone()), (2, c)]);
        i.gateway_preference = Some(n(2));
        assert!(i.hub_score(n(2)) > i.hub_score(n(1)));
    }

    #[test]
    fn link_cost_rewards_quality_and_history_and_penalizes_failures() {
        let mut link = CandidateLink::new(n(2), n(1));
        assert_eq!((link.a, link.b), (n(1), n(2)), "endpoints are canonical");
        assert_eq!(link.cost(), DEFAULT_LINK_COST);
        link.quality = Some(LinkQuality::new(100));
        let good = link.cost();
        link.quality = Some(LinkQuality::new(20));
        assert!(link.cost() > good);
        link.known_good = true;
        assert!(link.cost() < DEFAULT_LINK_COST + 16);
        link.stability.recent_failures = 3;
        assert!(link.cost() > 60);
        assert_eq!(link.connects(n(1)), Some(n(2)));
        assert_eq!(link.connects(n(3)), None);
    }

    #[test]
    fn candidates_with_unknown_endpoints_are_ignored() {
        let mut i = inputs(vec![
            (1, caps(true, None, None)),
            (2, caps(false, None, None)),
        ]);
        i.candidates = vec![
            CandidateLink::new(n(1), n(2)),
            CandidateLink::new(n(1), n(9)),
        ];
        assert_eq!(i.usable_candidates().count(), 1);
    }
}

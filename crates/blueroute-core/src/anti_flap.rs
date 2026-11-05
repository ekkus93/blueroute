//! Topology anti-flap policy (P9-006).
//!
//! Three independent brakes, all with injected time:
//! 1. Hysteresis: a different plan is adopted only if it is meaningfully better than the
//!    current one (score margin) and the minimum dwell time has passed; a plan that is simply
//!    invalid (a current link lost its candidate or capability) bypasses both.
//! 2. Failure backoff: a link that failed is excluded for an exponentially growing time.
//! 3. Known-good preference: links that have worked, and links currently active, are favored
//!    so noise cannot talk the planner out of a working connection.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::{NodeId, PlannedLink, TopologyInputs, TopologyPlan};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AntiFlapConfig {
    /// Minimum time between adopting two non-urgent plans.
    pub min_dwell: Duration,
    /// A new plan must score at least this much lower (better) than the current one.
    pub improvement_threshold: u64,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl Default for AntiFlapConfig {
    fn default() -> Self {
        Self {
            min_dwell: Duration::from_secs(30),
            improvement_threshold: 25,
            backoff_base: Duration::from_secs(5),
            backoff_max: Duration::from_secs(300),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Keep(KeepReason),
    Adopt(AdoptReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeepReason {
    NoChange,
    DwellNotElapsed,
    ImprovementBelowThreshold,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdoptReason {
    Initial,
    CurrentPlanInvalid,
    Improvement,
}

type Pair = (NodeId, NodeId);

fn pair_of(link: &PlannedLink) -> Pair {
    link.pair()
}

#[derive(Clone, Copy, Debug)]
struct Failure {
    count: u32,
    blocked_until: Duration,
}

#[derive(Debug)]
pub struct AntiFlap {
    config: AntiFlapConfig,
    last_adopted: Option<Duration>,
    failures: BTreeMap<Pair, Failure>,
    known_good: BTreeSet<Pair>,
}

impl AntiFlap {
    pub fn new(config: AntiFlapConfig) -> Self {
        Self {
            config,
            last_adopted: None,
            failures: BTreeMap::new(),
            known_good: BTreeSet::new(),
        }
    }

    pub fn record_failure(&mut self, link: &PlannedLink, now: Duration) {
        let entry = self.failures.entry(pair_of(link)).or_insert(Failure {
            count: 0,
            blocked_until: now,
        });
        entry.count += 1;
        let factor = 1u32 << (entry.count - 1).min(16);
        let wait = self
            .config
            .backoff_base
            .saturating_mul(factor)
            .min(self.config.backoff_max);
        entry.blocked_until = now + wait;
    }

    /// A link came up and worked: forget its failures and remember it as known-good.
    pub fn record_success(&mut self, link: &PlannedLink) {
        self.failures.remove(&pair_of(link));
        self.known_good.insert(pair_of(link));
    }

    pub fn is_blocked(&self, link: &PlannedLink, now: Duration) -> bool {
        self.is_pair_blocked(pair_of(link), now)
    }

    fn is_pair_blocked(&self, pair: Pair, now: Duration) -> bool {
        self.failures
            .get(&pair)
            .is_some_and(|f| now < f.blocked_until)
    }

    /// Shape planner inputs: drop backed-off candidates, expose failure counts, and favor
    /// known-good and currently active links.
    pub fn prepare_inputs(
        &self,
        inputs: &mut TopologyInputs,
        active: &BTreeSet<PlannedLink>,
        now: Duration,
    ) {
        let active_pairs: BTreeSet<Pair> = active.iter().map(pair_of).collect();
        inputs.candidates.retain(|c| {
            // An active link stays a candidate even if it recently failed; only new attempts wait.
            active_pairs.contains(&(c.a, c.b)) || !self.is_pair_blocked((c.a, c.b), now)
        });
        for candidate in &mut inputs.candidates {
            let pair = (candidate.a, candidate.b);
            candidate.known_good |= self.known_good.contains(&pair) || active_pairs.contains(&pair);
            candidate.stability.recent_failures = self.failures.get(&pair).map_or(0, |f| f.count);
        }
    }

    /// Lower is better: unlinked nodes dominate, then hub count, then total link cost.
    pub fn score(plan_links: &BTreeSet<PlannedLink>, inputs: &TopologyInputs) -> u64 {
        let linked: BTreeSet<NodeId> = plan_links.iter().flat_map(|l| [l.nap, l.panu]).collect();
        let unlinked = inputs.nodes.keys().filter(|n| !linked.contains(n)).count() as u64;
        let hubs = plan_links
            .iter()
            .map(|l| l.nap)
            .collect::<BTreeSet<_>>()
            .len() as u64;
        let cost: u64 = plan_links
            .iter()
            .map(|l| {
                inputs
                    .usable_candidates()
                    .find(|c| (c.a, c.b) == pair_of(l))
                    .map_or(u64::from(crate::DEFAULT_LINK_COST), |c| u64::from(c.cost()))
            })
            .sum();
        unlinked * 1000 + hubs * 100 + cost
    }

    fn plan_is_valid(current: &BTreeSet<PlannedLink>, inputs: &TopologyInputs) -> bool {
        current.iter().all(|link| {
            inputs.can_host(link.nap)
                && inputs.can_attach(link.panu)
                && inputs
                    .usable_candidates()
                    .any(|c| (c.a, c.b) == pair_of(link))
        })
    }

    /// Decide whether to move from the `current` links to the `desired` plan.
    pub fn decide(
        &self,
        current: &BTreeSet<PlannedLink>,
        desired: &TopologyPlan,
        inputs: &TopologyInputs,
        now: Duration,
    ) -> Decision {
        if *current == desired.links {
            return Decision::Keep(KeepReason::NoChange);
        }
        if current.is_empty() {
            return Decision::Adopt(AdoptReason::Initial);
        }
        if !Self::plan_is_valid(current, inputs) {
            return Decision::Adopt(AdoptReason::CurrentPlanInvalid);
        }
        if self
            .last_adopted
            .is_some_and(|t| now.saturating_sub(t) < self.config.min_dwell)
        {
            return Decision::Keep(KeepReason::DwellNotElapsed);
        }
        let current_score = Self::score(current, inputs);
        let desired_score = Self::score(&desired.links, inputs);
        if current_score >= desired_score + self.config.improvement_threshold {
            Decision::Adopt(AdoptReason::Improvement)
        } else {
            Decision::Keep(KeepReason::ImprovementBelowThreshold)
        }
    }

    /// Record that a new plan was adopted at `now` (starts the dwell timer).
    pub fn note_adopted(&mut self, now: Duration) {
        self.last_adopted = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CandidateLink, CapabilitySource, LinkQuality, NodeCapabilities, Sourced, select_roles,
    };

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn l(nap: u8, panu: u8) -> PlannedLink {
        PlannedLink {
            nap: n(nap),
            panu: n(panu),
        }
    }

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    fn src<T>(v: T) -> Sourced<T> {
        Sourced::new(v, CapabilitySource::Configured)
    }

    fn caps(nap: bool) -> NodeCapabilities {
        NodeCapabilities {
            panu: Some(src(true)),
            nap: Some(src(nap)),
            connection_policy_ceiling: Some(src(4_u16)),
            ..NodeCapabilities::default()
        }
    }

    /// Two equally capable hubs (1, 2) both adjacent to clients 3 and 4.
    fn inputs() -> TopologyInputs {
        TopologyInputs {
            nodes: [
                (n(1), caps(true)),
                (n(2), caps(true)),
                (n(3), caps(false)),
                (n(4), caps(false)),
            ]
            .into_iter()
            .collect(),
            candidates: vec![
                CandidateLink::new(n(1), n(3)),
                CandidateLink::new(n(1), n(4)),
                CandidateLink::new(n(2), n(3)),
                CandidateLink::new(n(2), n(4)),
                CandidateLink::new(n(1), n(2)),
            ],
            ..TopologyInputs::default()
        }
    }

    #[test]
    fn failure_backoff_grows_caps_and_clears_on_success() {
        let mut a = AntiFlap::new(AntiFlapConfig::default());
        let link = l(1, 2);
        assert!(!a.is_blocked(&link, s(0)));
        a.record_failure(&link, s(0));
        assert!(a.is_blocked(&link, s(4)) && !a.is_blocked(&link, s(5)));
        a.record_failure(&link, s(5));
        assert!(a.is_blocked(&link, s(14)) && !a.is_blocked(&link, s(15)));
        for t in 0..20 {
            a.record_failure(&link, s(100 + t));
        }
        assert!(
            !a.is_blocked(&link, s(100 + 19 + 300)),
            "capped at backoff_max"
        );
        a.record_success(&link);
        assert!(!a.is_blocked(&link, s(0)));
    }

    #[test]
    fn blocked_candidates_are_dropped_but_active_links_stay() {
        let mut a = AntiFlap::new(AntiFlapConfig::default());
        a.record_failure(&l(1, 3), s(0));
        a.record_failure(&l(1, 4), s(0));
        let mut i = inputs();
        a.prepare_inputs(&mut i, &BTreeSet::from([l(1, 4)]), s(1));
        let pairs: Vec<_> = i.candidates.iter().map(|c| (c.a, c.b)).collect();
        assert!(!pairs.contains(&(n(1), n(3))));
        assert!(pairs.contains(&(n(1), n(4))));
        let kept = i
            .candidates
            .iter()
            .find(|c| (c.a, c.b) == (n(1), n(4)))
            .unwrap();
        assert!(kept.known_good && kept.stability.recent_failures == 1);
    }

    #[test]
    fn known_good_links_win_otherwise_equal_choices() {
        let mut a = AntiFlap::new(AntiFlapConfig::default());
        let mut base = inputs();
        let default_hub = select_roles(&base).hubs;
        let other = if default_hub.contains(&n(1)) { 2 } else { 1 };
        a.record_success(&l(other, 3));
        a.record_success(&l(other, 4));
        a.prepare_inputs(&mut base, &BTreeSet::new(), s(0));
        assert_eq!(select_roles(&base).hubs, BTreeSet::from([n(other)]));
    }

    #[test]
    fn noisy_metrics_do_not_cause_continuous_rebuilds() {
        let mut a = AntiFlap::new(AntiFlapConfig::default());
        let mut current = BTreeSet::new();
        let mut adoptions = 0;
        let mut seed = 12345_u64;
        for round in 0..300_u64 {
            let mut i = inputs();
            // Jitter every link quality randomly between 40 and 100.
            for c in &mut i.candidates {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                c.quality = Some(LinkQuality::new(40 + ((seed >> 33) % 61) as u8));
            }
            a.prepare_inputs(&mut i, &current, s(round * 5));
            let desired = select_roles(&i);
            if let Decision::Adopt(_) = a.decide(&current, &desired, &i, s(round * 5)) {
                adoptions += 1;
                current = desired.links;
                a.note_adopted(s(round * 5));
            }
        }
        assert!(adoptions <= 2, "{adoptions} adoptions under noise");
        assert!(!current.is_empty());
    }

    #[test]
    fn decide_reports_why_it_keeps_or_adopts() {
        let mut a = AntiFlap::new(AntiFlapConfig::default());
        let i = inputs();
        let star1 = TopologyPlan {
            links: BTreeSet::from([l(1, 3), l(1, 4), l(1, 2)]),
            ..TopologyPlan::default()
        };
        assert_eq!(
            a.decide(&star1.links, &star1, &i, s(0)),
            Decision::Keep(KeepReason::NoChange)
        );
        assert_eq!(
            a.decide(&BTreeSet::new(), &star1, &i, s(0)),
            Decision::Adopt(AdoptReason::Initial)
        );

        // Valid current plan, equal-score alternative: below threshold.
        let star2 = TopologyPlan {
            links: BTreeSet::from([l(2, 3), l(2, 4), l(2, 1)]),
            ..TopologyPlan::default()
        };
        assert_eq!(
            a.decide(&star1.links, &star2, &i, s(1000)),
            Decision::Keep(KeepReason::ImprovementBelowThreshold)
        );

        // Much better plan is held back during dwell, then adopted.
        let poor = BTreeSet::from([l(1, 3)]);
        a.note_adopted(s(1000));
        assert_eq!(
            a.decide(&poor, &star1, &i, s(1010)),
            Decision::Keep(KeepReason::DwellNotElapsed)
        );
        assert_eq!(
            a.decide(&poor, &star1, &i, s(1031)),
            Decision::Adopt(AdoptReason::Improvement)
        );

        // A current link that lost its candidate is urgent: bypasses dwell.
        let mut gone = inputs();
        gone.candidates.retain(|c| (c.a, c.b) != (n(1), n(3)));
        assert_eq!(
            a.decide(&star1.links, &star2, &gone, s(1001)),
            Decision::Adopt(AdoptReason::CurrentPlanInvalid)
        );
    }
}

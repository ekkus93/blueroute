//! Topology plan diff (P9-003).
//!
//! Compares the active links with a desired plan and emits the minimal ordered change list.
//! Unchanged links are never touched. Where per-node capacity allows, a replacement link is
//! created before the obsolete one is removed (make-before-break); when a node is saturated,
//! only the removals that free the needed capacity are moved ahead of the blocked addition.

use std::collections::BTreeSet;

use crate::{NodeId, PlannedLink, TopologyPlan};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkChange {
    Add(PlannedLink),
    Remove(PlannedLink),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlanDiff {
    /// Apply in order.
    pub changes: Vec<LinkChange>,
    /// Desired links that cannot be added without exceeding capacity (plan/capacity mismatch).
    pub blocked: Vec<PlannedLink>,
}

impl PlanDiff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.blocked.is_empty()
    }
}

fn degree(links: &BTreeSet<PlannedLink>, node: NodeId) -> usize {
    links
        .iter()
        .filter(|l| l.nap == node || l.panu == node)
        .count()
}

pub fn diff_topology(
    active: &BTreeSet<PlannedLink>,
    desired: &TopologyPlan,
    capacity: &dyn Fn(NodeId) -> u16,
) -> PlanDiff {
    let mut current = active.clone();
    let mut adds: Vec<PlannedLink> = desired.links.difference(active).copied().collect();
    let mut removes: Vec<PlannedLink> = active.difference(&desired.links).copied().collect();
    let mut diff = PlanDiff::default();

    let can_add = |current: &BTreeSet<PlannedLink>, link: &PlannedLink| {
        let pair_taken = current.iter().any(|l| l.pair() == link.pair());
        !pair_taken
            && [link.nap, link.panu]
                .iter()
                .all(|n| degree(current, *n) < usize::from(capacity(*n)))
    };

    loop {
        if let Some(i) = adds.iter().position(|a| can_add(&current, a)) {
            let link = adds.remove(i);
            current.insert(link);
            diff.changes.push(LinkChange::Add(link));
            continue;
        }
        // Blocked additions: free capacity (or the pair itself) with the fewest removals.
        let blocking = adds.first().copied();
        let Some(blocked) = blocking else {
            break;
        };
        let relevant = removes.iter().position(|r| {
            r.pair() == blocked.pair()
                || [blocked.nap, blocked.panu].iter().any(|n| {
                    (r.nap == *n || r.panu == *n)
                        && degree(&current, *n) >= usize::from(capacity(*n))
                })
        });
        match relevant {
            Some(i) => {
                let link = removes.remove(i);
                current.remove(&link);
                diff.changes.push(LinkChange::Remove(link));
            }
            None => {
                diff.blocked.append(&mut adds);
                break;
            }
        }
    }
    for link in removes {
        diff.changes.push(LinkChange::Remove(link));
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CandidateLink, CapabilitySource, NodeCapabilities, Sourced, TopologyInputs, select_roles,
    };
    use LinkChange::{Add, Remove};

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn l(nap: u8, panu: u8) -> PlannedLink {
        PlannedLink {
            nap: n(nap),
            panu: n(panu),
        }
    }

    fn plan(links: &[PlannedLink]) -> TopologyPlan {
        TopologyPlan {
            hubs: links.iter().map(|l| l.nap).collect(),
            links: links.iter().copied().collect(),
            unassigned: BTreeSet::new(),
        }
    }

    fn set(links: &[PlannedLink]) -> BTreeSet<PlannedLink> {
        links.iter().copied().collect()
    }

    fn src<T>(v: T) -> Sourced<T> {
        Sourced::new(v, CapabilitySource::Configured)
    }

    #[test]
    fn identical_topology_produces_no_changes() {
        let links = [l(1, 2), l(1, 3)];
        assert!(diff_topology(&set(&links), &plan(&links), &|_| 4).is_empty());
    }

    #[test]
    fn replacement_is_created_before_the_old_link_is_removed_when_capacity_allows() {
        let diff = diff_topology(&set(&[l(1, 2)]), &plan(&[l(1, 3)]), &|_| 2);
        assert_eq!(diff.changes, vec![Add(l(1, 3)), Remove(l(1, 2))]);
    }

    #[test]
    fn saturated_nodes_remove_only_what_frees_capacity_first() {
        let active = set(&[l(1, 2), l(1, 3), l(4, 5)]);
        // Hub 1 holds 2 links; swap client 3 for 6 while 4-5 is also dropped for 4-7.
        let desired = plan(&[l(1, 2), l(1, 6), l(4, 7)]);
        let diff = diff_topology(&active, &desired, &|_| 2);
        let pos = |c: LinkChange| diff.changes.iter().position(|x| *x == c).unwrap();
        assert!(
            pos(Remove(l(1, 3))) < pos(Add(l(1, 6))),
            "capacity forces break-before-make"
        );
        assert!(
            pos(Add(l(4, 7))) < pos(Remove(l(4, 5))),
            "free capacity keeps make-before-break"
        );
        assert_eq!(diff.changes.len(), 4);
        assert!(diff.blocked.is_empty());
        assert!(
            !diff.changes.contains(&Remove(l(1, 2))),
            "unchanged links are untouched"
        );
    }

    #[test]
    fn a_role_flip_on_the_same_pair_removes_then_adds() {
        let diff = diff_topology(&set(&[l(1, 2)]), &plan(&[l(2, 1)]), &|_| 4);
        assert_eq!(diff.changes, vec![Remove(l(1, 2)), Add(l(2, 1))]);
    }

    #[test]
    fn infeasible_additions_are_reported_not_forced() {
        let diff = diff_topology(&BTreeSet::new(), &plan(&[l(1, 2), l(1, 3)]), &|_| 1);
        assert_eq!(diff.changes, vec![Add(l(1, 2))]);
        assert_eq!(diff.blocked, vec![l(1, 3)]);
    }

    #[test]
    fn applying_the_diff_reaches_the_desired_plan() {
        let active = set(&[l(1, 2), l(1, 3), l(4, 5), l(6, 7)]);
        let desired = plan(&[l(1, 2), l(1, 8), l(4, 5), l(9, 7)]);
        let diff = diff_topology(&active, &desired, &|_| 2);
        let mut state = active;
        for change in diff.changes {
            match change {
                Add(link) => assert!(state.insert(link)),
                Remove(link) => assert!(state.remove(&link)),
            }
        }
        assert_eq!(state, desired.links);
    }

    #[test]
    fn small_metric_changes_do_not_rebuild_the_network() {
        let node = |nap| NodeCapabilities {
            panu: Some(src(true)),
            nap: Some(src(nap)),
            connection_policy_ceiling: Some(src(4_u16)),
            ..NodeCapabilities::default()
        };
        let mut inputs = TopologyInputs {
            nodes: [
                (n(1), node(true)),
                (n(2), node(false)),
                (n(3), node(false)),
                (n(4), node(false)),
            ]
            .into_iter()
            .collect(),
            candidates: vec![
                CandidateLink::new(n(1), n(2)),
                CandidateLink::new(n(1), n(3)),
                CandidateLink::new(n(1), n(4)),
                CandidateLink::new(n(2), n(3)),
            ],
            ..TopologyInputs::default()
        };
        let before = select_roles(&inputs);
        for candidate in &mut inputs.candidates {
            candidate.quality = Some(crate::LinkQuality::new(80));
        }
        let after = select_roles(&inputs);
        assert!(diff_topology(&before.links, &after, &|id| inputs.capacity(id)).is_empty());
    }
}

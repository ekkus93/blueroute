//! NAP role selection v1 (P9-002).
//!
//! A deterministic greedy planner. Each round picks the host-capable node that can attach the
//! most still-unlinked clients within its capacity (ties: better hub score, then lowest
//! `NodeId`), so hubs are created only when they link someone new. A node that is already a
//! client may become a hub for further nodes, which yields interconnected stars. Separate
//! groups are then joined with the cheapest feasible candidate links. Nodes that cannot host
//! (client-only) are never hubs; nodes nothing can reach stay unassigned.

use std::collections::{BTreeMap, BTreeSet};

use crate::{CandidateLink, NodeId, TopologyInputs};

/// One planned BNEP relationship: `nap` hosts, `panu` attaches.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlannedLink {
    pub nap: NodeId,
    pub panu: NodeId,
}

impl PlannedLink {
    pub fn pair(&self) -> (NodeId, NodeId) {
        if self.nap <= self.panu {
            (self.nap, self.panu)
        } else {
            (self.panu, self.nap)
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TopologyPlan {
    pub hubs: BTreeSet<NodeId>,
    pub links: BTreeSet<PlannedLink>,
    /// Known nodes that received no link.
    pub unassigned: BTreeSet<NodeId>,
}

struct State<'a> {
    inputs: &'a TopologyInputs,
    adjacency: BTreeMap<NodeId, Vec<&'a CandidateLink>>,
    load: BTreeMap<NodeId, u16>,
    links: BTreeSet<PlannedLink>,
}

impl<'a> State<'a> {
    fn new(inputs: &'a TopologyInputs) -> Self {
        let mut adjacency: BTreeMap<NodeId, Vec<&CandidateLink>> = BTreeMap::new();
        for link in inputs.usable_candidates() {
            adjacency.entry(link.a).or_default().push(link);
            adjacency.entry(link.b).or_default().push(link);
        }
        for list in adjacency.values_mut() {
            list.sort_by_key(|l| (l.cost(), l.a, l.b));
        }
        Self {
            inputs,
            adjacency,
            load: BTreeMap::new(),
            links: BTreeSet::new(),
        }
    }

    fn free(&self, node: NodeId) -> u16 {
        self.inputs
            .capacity(node)
            .saturating_sub(self.load.get(&node).copied().unwrap_or(0))
    }

    fn linked(&self, node: NodeId) -> bool {
        self.load.get(&node).is_some_and(|l| *l > 0)
    }

    fn already_paired(&self, x: NodeId, y: NodeId) -> bool {
        self.links
            .iter()
            .any(|l| l.pair() == if x <= y { (x, y) } else { (y, x) })
    }

    fn add(&mut self, nap: NodeId, panu: NodeId) {
        self.links.insert(PlannedLink { nap, panu });
        *self.load.entry(nap).or_default() += 1;
        *self.load.entry(panu).or_default() += 1;
    }

    /// Clients `hub` could take now: unlinked, attachable, with spare capacity, cheapest first.
    fn coverable(&self, hub: NodeId) -> Vec<(NodeId, u32)> {
        let mut out: Vec<(NodeId, u32)> = Vec::new();
        for link in self.adjacency.get(&hub).into_iter().flatten() {
            let Some(other) = link.connects(hub) else {
                continue;
            };
            if !self.linked(other)
                && self.inputs.can_attach(other)
                && self.free(other) > 0
                && !self.already_paired(hub, other)
                && !out.iter().any(|(n, _)| *n == other)
            {
                out.push((other, link.cost()));
            }
        }
        out
    }
}

pub fn select_roles(inputs: &TopologyInputs) -> TopologyPlan {
    let mut plan = TopologyPlan::default();
    if inputs.policy.automatic_role_selection {
        let mut state = State::new(inputs);
        assign_clients(&mut state);
        join_groups(&mut state);
        plan.links = state.links.clone();
        plan.hubs = state.links.iter().map(|l| l.nap).collect();
    }
    let linked: BTreeSet<NodeId> = plan.links.iter().flat_map(|l| [l.nap, l.panu]).collect();
    plan.unassigned = inputs
        .nodes
        .keys()
        .copied()
        .filter(|n| !linked.contains(n))
        .collect();
    plan
}

fn assign_clients(state: &mut State<'_>) {
    loop {
        // (clients covered, hub score, total link cost, hub, clients)
        let mut best: Option<(usize, i64, u64, NodeId, Vec<NodeId>)> = None;
        for hub in state.inputs.nodes.keys().copied() {
            if !state.inputs.can_host(hub) || state.free(hub) == 0 {
                continue;
            }
            let mut clients = state.coverable(hub);
            clients.truncate(usize::from(state.free(hub)));
            if clients.is_empty() {
                continue;
            }
            let cost: u64 = clients.iter().map(|(_, c)| u64::from(*c)).sum();
            let score = state.inputs.hub_score(hub);
            let better = match &best {
                None => true,
                Some((n, bs, bc, id, _)) => {
                    // More coverage, then better hub, then cheaper links, then lowest id.
                    (
                        clients.len(),
                        score,
                        std::cmp::Reverse(cost),
                        std::cmp::Reverse(hub),
                    ) > (*n, *bs, std::cmp::Reverse(*bc), std::cmp::Reverse(*id))
                }
            };
            if better {
                let covered = clients.len();
                let ids: Vec<NodeId> = clients.into_iter().map(|(id, _)| id).collect();
                best = Some((covered, score, cost, hub, ids));
            }
        }
        let Some((_, _, _, hub, clients)) = best else {
            return;
        };
        for client in clients {
            state.add(hub, client);
        }
    }
}

/// Join disconnected groups with the cheapest feasible candidate links (Kruskal order).
fn join_groups(state: &mut State<'_>) {
    let mut parent: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    fn find(parent: &mut BTreeMap<NodeId, NodeId>, x: NodeId) -> NodeId {
        let p = *parent.entry(x).or_insert(x);
        if p == x {
            x
        } else {
            let root = find(parent, p);
            parent.insert(x, root);
            root
        }
    }
    for link in state.links.clone() {
        let (a, b) = (find(&mut parent, link.nap), find(&mut parent, link.panu));
        parent.insert(a.max(b), a.min(b));
    }
    let mut candidates: Vec<&CandidateLink> = state.inputs.usable_candidates().collect();
    candidates.sort_by_key(|l| (l.cost(), l.a, l.b));
    for link in candidates {
        let (ra, rb) = (find(&mut parent, link.a), find(&mut parent, link.b));
        // Only join groups that already contain links; isolated singletons were offered to
        // every hub in `assign_clients`.
        if ra == rb || !state.linked(link.a) || !state.linked(link.b) {
            continue;
        }
        let orient = |nap: NodeId, panu: NodeId| {
            state.inputs.can_host(nap)
                && state.inputs.can_attach(panu)
                && state.free(nap) > 0
                && state.free(panu) > 0
        };
        let choice = match (orient(link.a, link.b), orient(link.b, link.a)) {
            (true, true) => {
                if state.inputs.hub_score(link.a) >= state.inputs.hub_score(link.b) {
                    Some((link.a, link.b))
                } else {
                    Some((link.b, link.a))
                }
            }
            (true, false) => Some((link.a, link.b)),
            (false, true) => Some((link.b, link.a)),
            (false, false) => None,
        };
        if let Some((nap, panu)) = choice {
            state.add(nap, panu);
            parent.insert(ra.max(rb), ra.min(rb));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilitySource, NodeCapabilities, PowerState, Sourced};

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn src<T>(v: T) -> Sourced<T> {
        Sourced::new(v, CapabilitySource::Configured)
    }

    fn node(nap: bool, ceiling: Option<u16>, ac: bool) -> NodeCapabilities {
        NodeCapabilities {
            panu: Some(src(true)),
            nap: Some(src(nap)),
            connection_policy_ceiling: ceiling.map(src),
            power_state: ac.then(|| src(PowerState::AcPowered)),
            ..NodeCapabilities::default()
        }
    }

    fn inputs(nodes: &[(u8, NodeCapabilities)], edges: &[(u8, u8)]) -> TopologyInputs {
        TopologyInputs {
            nodes: nodes.iter().map(|(i, c)| (n(*i), c.clone())).collect(),
            candidates: edges
                .iter()
                .map(|(a, b)| CandidateLink::new(n(*a), n(*b)))
                .collect(),
            ..TopologyInputs::default()
        }
    }

    fn links(plan: &TopologyPlan) -> Vec<(u8, u8)> {
        plan.links
            .iter()
            .map(|l| (l.nap.as_bytes()[0], l.panu.as_bytes()[0]))
            .collect()
    }

    #[test]
    fn one_capable_hub_serves_client_only_nodes() {
        let i = inputs(
            &[
                (1, node(true, Some(4), true)),
                (2, node(false, None, false)),
                (3, node(false, None, false)),
                (4, node(false, None, false)),
            ],
            &[(1, 2), (1, 3), (1, 4), (2, 3)],
        );
        let plan = select_roles(&i);
        assert_eq!(plan.hubs, BTreeSet::from([n(1)]));
        assert_eq!(links(&plan), vec![(1, 2), (1, 3), (1, 4)]);
        assert!(plan.unassigned.is_empty());
    }

    #[test]
    fn unnecessary_hubs_are_avoided_in_a_fully_capable_mesh() {
        let nodes: Vec<_> = (1..=4).map(|i| (i, node(true, Some(3), false))).collect();
        let edges = [(1, 2), (1, 3), (1, 4), (2, 3), (2, 4), (3, 4)];
        let plan = select_roles(&inputs(&nodes, &edges));
        assert_eq!(plan.hubs.len(), 1);
        assert_eq!(plan.links.len(), 3);
    }

    #[test]
    fn capacity_is_respected_and_a_client_becomes_a_second_hub_to_reach_the_rest() {
        // A can hold 2 links; D is only reachable via B, which can host.
        let i = inputs(
            &[
                (1, node(true, Some(2), true)),
                (2, node(true, Some(3), false)),
                (3, node(false, None, false)),
                (4, node(false, None, false)),
            ],
            &[(1, 2), (1, 3), (2, 4)],
        );
        let plan = select_roles(&i);
        assert_eq!(plan.hubs, BTreeSet::from([n(1), n(2)]));
        assert_eq!(links(&plan), vec![(1, 2), (1, 3), (2, 4)]);
        for (id, caps) in &i.nodes {
            let used = plan
                .links
                .iter()
                .filter(|l| l.nap == *id || l.panu == *id)
                .count();
            assert!(used <= usize::from(i.capacity(*id)), "{id} over capacity");
            let _ = caps;
        }
    }

    #[test]
    fn nodes_nothing_can_reach_stay_unassigned_and_client_only_never_hosts() {
        let i = inputs(
            &[
                (1, node(false, Some(8), true)),
                (2, node(false, None, false)),
                (3, node(true, None, false)),
            ],
            &[(1, 2)],
        );
        let plan = select_roles(&i);
        assert!(plan.hubs.is_empty());
        assert_eq!(plan.unassigned, BTreeSet::from([n(1), n(2), n(3)]));
    }

    #[test]
    fn unknown_capacity_is_conservative() {
        let i = inputs(
            &[
                (1, node(true, None, false)),
                (2, node(false, None, false)),
                (3, node(false, None, false)),
            ],
            &[(1, 2), (1, 3)],
        );
        let plan = select_roles(&i);
        assert_eq!(plan.links.len(), 1, "default capacity is one link");
        assert_eq!(plan.unassigned.len(), 1);
    }

    #[test]
    fn policy_can_cap_links_or_disable_automatic_selection() {
        let mut i = inputs(
            &[
                (1, node(true, Some(8), true)),
                (2, node(false, None, false)),
                (3, node(false, None, false)),
            ],
            &[(1, 2), (1, 3)],
        );
        i.policy.max_active_links = Some(1);
        assert_eq!(select_roles(&i).links.len(), 1);
        i.policy.automatic_role_selection = false;
        let off = select_roles(&i);
        assert!(off.links.is_empty() && off.hubs.is_empty());
        assert_eq!(off.unassigned.len(), 3);
    }

    #[test]
    fn separate_groups_are_joined_when_capacity_allows() {
        let i = inputs(
            &[
                (1, node(true, Some(3), true)),
                (2, node(false, None, false)),
                (3, node(true, Some(3), true)),
                (4, node(false, None, false)),
            ],
            &[(1, 2), (3, 4), (1, 3)],
        );
        let plan = select_roles(&i);
        assert_eq!(plan.links.len(), 3);
        assert!(plan.unassigned.is_empty());
        assert!(links(&plan).iter().any(|l| *l == (1, 3) || *l == (3, 1)));
    }

    #[test]
    fn result_does_not_depend_on_input_order() {
        let nodes: Vec<_> = (1..=5)
            .map(|i| (i, node(i % 2 == 1, Some(2), i == 1)))
            .collect();
        let edges = vec![(1, 2), (1, 3), (2, 3), (3, 4), (4, 5), (3, 5)];
        let forward = select_roles(&inputs(&nodes, &edges));
        let mut reversed_nodes = nodes.clone();
        reversed_nodes.reverse();
        let mut reversed_edges: Vec<_> = edges.iter().rev().map(|(a, b)| (*b, *a)).collect();
        reversed_edges.rotate_left(2);
        assert_eq!(
            forward,
            select_roles(&inputs(&reversed_nodes, &reversed_edges))
        );
    }

    #[test]
    fn better_links_and_ac_power_win_ties() {
        let mut i = inputs(
            &[
                (1, node(true, Some(1), false)),
                (2, node(true, Some(1), true)),
                (3, node(false, None, false)),
            ],
            &[(1, 3), (2, 3)],
        );
        assert_eq!(
            select_roles(&i).hubs,
            BTreeSet::from([n(2)]),
            "AC power preferred"
        );
        i.policy.prefer_ac_powered_hubs = false;
        assert_eq!(
            select_roles(&i).hubs,
            BTreeSet::from([n(1)]),
            "then lowest NodeId"
        );
    }
}

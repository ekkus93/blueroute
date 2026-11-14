use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{CoreError, ErrorKind, LinkId, NodeCapabilities, NodeId, SegmentId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    Planned,
    Connecting,
    Active,
    Disconnected,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkHealth {
    Unknown,
    Healthy,
    Degraded,
    Failed,
}

/// User/data-plane reachability class derived from the active PAN graph.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    Local,
    DirectLink,
    SamePanSegment,
    Routed,
    Unreachable,
}

/// One BNEP/PAN relationship. The NAP and PANU roles are explicit internally.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanLink {
    pub id: LinkId,
    pub segment_id: SegmentId,
    pub nap: NodeId,
    pub panu: NodeId,
    pub state: LinkState,
    pub health: LinkHealth,
    pub cost: u32,
}

impl PanLink {
    pub fn is_forwarding_candidate(&self) -> bool {
        self.state == LinkState::Active && self.health != LinkHealth::Failed
    }

    pub fn other_endpoint(&self, node: NodeId) -> Option<NodeId> {
        if node == self.nap {
            Some(self.panu)
        } else if node == self.panu {
            Some(self.nap)
        } else {
            None
        }
    }
}

/// One NAP's PAN segment. Its members are always the NAP, the PANU of every link in the segment,
/// and any member added explicitly with [`PanSegment::add_member`] before the segment is inserted.
/// Explicit members must be nodes of the graph; they stay members until removed from the graph,
/// whatever happens to links. The graph keeps `members` in step with its links.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanSegment {
    pub id: SegmentId,
    pub nap: NodeId,
    members: BTreeSet<NodeId>,
    explicit: BTreeSet<NodeId>,
}

impl PanSegment {
    pub fn new(id: SegmentId, nap: NodeId) -> Self {
        let mut members = BTreeSet::new();
        members.insert(nap);
        Self {
            id,
            nap,
            members,
            explicit: BTreeSet::new(),
        }
    }

    /// Add a member that has no link (yet). Returns whether it was new.
    pub fn add_member(&mut self, node: NodeId) -> bool {
        self.explicit.insert(node);
        self.members.insert(node)
    }

    pub fn members(&self) -> impl Iterator<Item = &NodeId> {
        self.members.iter()
    }

    /// Whether `node` stays a member regardless of links: the NAP or an explicit member.
    fn keeps(&self, node: &NodeId) -> bool {
        *node == self.nap || self.explicit.contains(node)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TopologyGraph {
    nodes: BTreeMap<NodeId, NodeCapabilities>,
    links: BTreeMap<LinkId, PanLink>,
    segments: BTreeMap<SegmentId, PanSegment>,
}

impl TopologyGraph {
    pub fn add_node(&mut self, id: NodeId, capabilities: NodeCapabilities) -> bool {
        self.nodes.insert(id, capabilities).is_none()
    }

    pub fn remove_node(&mut self, id: NodeId) -> Option<NodeCapabilities> {
        let removed = self.nodes.remove(&id)?;
        self.links
            .retain(|_, link| link.nap != id && link.panu != id);
        self.segments.retain(|_, segment| segment.nap != id);
        for segment in self.segments.values_mut() {
            segment.members.remove(&id);
            segment.explicit.remove(&id);
        }
        Some(removed)
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeCapabilities> {
        self.nodes.get(id)
    }

    /// Insert a segment. `Ok(true)`: a new segment. `Ok(false)`: the segment already exists with
    /// the same NAP; it is kept, with its link members, and any new explicit members are added.
    /// A different NAP for an existing SegmentId is refused (its links would no longer match),
    /// as are a NAP or explicit member that is not a node of the graph. A refused call changes
    /// nothing (CR-F3, GRAPH-1/2/4).
    pub fn add_segment(&mut self, segment: PanSegment) -> Result<bool, CoreError> {
        if !self.nodes.contains_key(&segment.nap) {
            return Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN segment NAP is not present in the topology",
            ));
        }
        if segment
            .explicit
            .iter()
            .any(|member| !self.nodes.contains_key(member))
        {
            return Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN segment member is not present in the topology",
            ));
        }
        match self.segments.get_mut(&segment.id) {
            Some(existing) if existing.nap != segment.nap => Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN segment already exists with a different NAP",
            )),
            Some(existing) => {
                for member in segment.explicit {
                    existing.explicit.insert(member);
                    existing.members.insert(member);
                }
                Ok(false)
            }
            None => {
                self.segments.insert(segment.id, segment);
                Ok(true)
            }
        }
    }

    /// Insert a link. `Ok(true)`: a new link, whose PANU becomes a segment member. `Ok(false)`:
    /// the LinkId exists with the same segment, NAP and PANU, and only its state, health and cost
    /// are updated. The same LinkId with a different segment or endpoint is refused rather than
    /// rebound, as are unknown endpoints or segment and a NAP that is not the segment's. Every
    /// check runs before anything changes, so a refused call changes nothing (CR-F3).
    pub fn add_link(&mut self, link: PanLink) -> Result<bool, CoreError> {
        if link.nap == link.panu {
            return Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN link endpoints must be different nodes",
            ));
        }
        if !self.nodes.contains_key(&link.nap) || !self.nodes.contains_key(&link.panu) {
            return Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN link endpoint is not present in the topology",
            ));
        }
        let segment = self.segments.get_mut(&link.segment_id).ok_or_else(|| {
            CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN link references an unknown segment",
            )
        })?;
        if segment.nap != link.nap {
            return Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN link NAP does not match the segment NAP",
            ));
        }
        if let Some(existing) = self.links.get_mut(&link.id) {
            if existing.segment_id != link.segment_id
                || existing.nap != link.nap
                || existing.panu != link.panu
            {
                return Err(CoreError::new(
                    ErrorKind::TopologyFailure,
                    "PAN link already exists with different endpoints or segment",
                ));
            }
            existing.state = link.state;
            existing.health = link.health;
            existing.cost = link.cost;
            return Ok(false);
        }
        segment.members.insert(link.panu);
        self.links.insert(link.id, link);
        Ok(true)
    }

    pub fn remove_link(&mut self, id: &LinkId) -> Option<PanLink> {
        let removed = self.links.remove(id)?;
        if let Some(segment) = self.segments.get_mut(&removed.segment_id) {
            let still_attached = segment.keeps(&removed.panu)
                || self
                    .links
                    .values()
                    .any(|link| link.segment_id == removed.segment_id && link.panu == removed.panu);
            if !still_attached {
                segment.members.remove(&removed.panu);
            }
        }
        Some(removed)
    }

    pub fn direct_neighbors(&self, node: NodeId) -> BTreeSet<NodeId> {
        self.links
            .values()
            .filter(|link| link.is_forwarding_candidate())
            .filter_map(|link| link.other_endpoint(node))
            .collect()
    }

    /// Returns a deterministic minimum-hop path through active links, including both endpoints.
    pub fn shortest_path(&self, start: NodeId, end: NodeId) -> Option<Vec<NodeId>> {
        if !self.nodes.contains_key(&start) || !self.nodes.contains_key(&end) {
            return None;
        }
        if start == end {
            return Some(vec![start]);
        }

        let mut queue = VecDeque::new();
        queue.push_back(start);
        let mut previous = BTreeMap::<NodeId, NodeId>::new();
        let mut visited = BTreeSet::new();
        visited.insert(start);

        while let Some(current) = queue.pop_front() {
            for neighbor in self.direct_neighbors(current) {
                if !visited.insert(neighbor) {
                    continue;
                }
                previous.insert(neighbor, current);
                if neighbor == end {
                    let mut path = vec![end];
                    let mut cursor = end;
                    while let Some(parent) = previous.get(&cursor).copied() {
                        path.push(parent);
                        if parent == start {
                            path.reverse();
                            return Some(path);
                        }
                        cursor = parent;
                    }
                }
                queue.push_back(neighbor);
            }
        }
        None
    }

    pub fn is_directly_reachable(&self, start: NodeId, end: NodeId) -> bool {
        self.direct_neighbors(start).contains(&end)
    }

    fn node_is_active_in_segment(&self, node: NodeId, segment_id: SegmentId) -> bool {
        let Some(segment) = self.segments.get(&segment_id) else {
            return false;
        };
        node == segment.nap
            || self.links.values().any(|link| {
                link.segment_id == segment_id && link.panu == node && link.is_forwarding_candidate()
            })
    }

    pub fn reachability(&self, start: NodeId, end: NodeId) -> Reachability {
        let Some(path) = self.shortest_path(start, end) else {
            return Reachability::Unreachable;
        };
        if path.len() == 1 {
            return Reachability::Local;
        }
        if path.len() == 2 {
            return Reachability::DirectLink;
        }

        let same_active_segment = self.segments.values().any(|segment| {
            self.node_is_active_in_segment(start, segment.id)
                && self.node_is_active_in_segment(end, segment.id)
        });

        if same_active_segment {
            Reachability::SamePanSegment
        } else {
            Reachability::Routed
        }
    }

    pub fn is_routed_reachable(&self, start: NodeId, end: NodeId) -> bool {
        self.reachability(start, end) == Reachability::Routed
    }

    pub fn nodes(&self) -> impl Iterator<Item = (&NodeId, &NodeCapabilities)> {
        self.nodes.iter()
    }

    pub fn links(&self) -> impl Iterator<Item = (&LinkId, &PanLink)> {
        self.links.iter()
    }

    pub fn segments(&self) -> impl Iterator<Item = (&SegmentId, &PanSegment)> {
        self.segments.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilitySource, Sourced};

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn link(value: u8) -> LinkId {
        LinkId::from_bytes([value; 16])
    }

    fn segment(value: u8) -> SegmentId {
        SegmentId::from_bytes([value; 16])
    }

    fn active_link(id: u8, segment_id: u8, nap: u8, panu: u8) -> PanLink {
        PanLink {
            id: link(id),
            segment_id: segment(segment_id),
            nap: node(nap),
            panu: node(panu),
            state: LinkState::Active,
            health: LinkHealth::Healthy,
            cost: 1,
        }
    }

    fn add_node(graph: &mut TopologyGraph, value: u8, nap: bool) {
        graph.add_node(
            node(value),
            NodeCapabilities {
                nap: Some(Sourced::new(nap, CapabilitySource::Measured)),
                panu: Some(Sourced::new(true, CapabilitySource::Measured)),
                ..NodeCapabilities::default()
            },
        );
    }

    #[test]
    fn single_star_is_representable() {
        let mut graph = TopologyGraph::default();
        add_node(&mut graph, 1, true);
        add_node(&mut graph, 2, false);
        add_node(&mut graph, 3, false);
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        graph.add_link(active_link(1, 1, 1, 2)).unwrap();
        graph.add_link(active_link(2, 1, 1, 3)).unwrap();

        assert!(graph.is_directly_reachable(node(1), node(2)));
        assert_eq!(
            graph.reachability(node(2), node(3)),
            Reachability::SamePanSegment
        );
        assert_eq!(
            graph.shortest_path(node(2), node(3)).unwrap(),
            vec![node(2), node(1), node(3)]
        );
    }

    #[test]
    fn routed_multi_star_graph_is_representable() {
        let mut graph = TopologyGraph::default();
        for value in 1..=4 {
            add_node(&mut graph, value, value == 1 || value == 3);
        }
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        graph
            .add_segment(PanSegment::new(segment(2), node(3)))
            .unwrap();
        graph.add_link(active_link(1, 1, 1, 2)).unwrap();
        graph.add_link(active_link(2, 1, 1, 3)).unwrap();
        graph.add_link(active_link(3, 2, 3, 4)).unwrap();

        assert_eq!(
            graph.shortest_path(node(2), node(4)).unwrap(),
            vec![node(2), node(1), node(3), node(4)]
        );
        assert_eq!(graph.reachability(node(2), node(4)), Reachability::Routed);
    }

    #[test]
    fn failed_link_partitions_graph() {
        let mut graph = TopologyGraph::default();
        add_node(&mut graph, 1, true);
        add_node(&mut graph, 2, false);
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        let mut failed = active_link(1, 1, 1, 2);
        failed.health = LinkHealth::Failed;
        graph.add_link(failed).unwrap();
        assert!(graph.shortest_path(node(1), node(2)).is_none());
        assert_eq!(
            graph.reachability(node(1), node(2)),
            Reachability::Unreachable
        );
    }

    #[test]
    fn redundant_paths_are_deterministic() {
        let mut graph = TopologyGraph::default();
        for value in 1..=4 {
            add_node(&mut graph, value, value == 1 || value == 2 || value == 3);
        }
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        graph
            .add_segment(PanSegment::new(segment(2), node(2)))
            .unwrap();
        graph
            .add_segment(PanSegment::new(segment(3), node(3)))
            .unwrap();
        graph.add_link(active_link(1, 1, 1, 2)).unwrap();
        graph.add_link(active_link(2, 1, 1, 3)).unwrap();
        graph.add_link(active_link(3, 2, 2, 4)).unwrap();
        graph.add_link(active_link(4, 3, 3, 4)).unwrap();

        assert_eq!(
            graph.shortest_path(node(1), node(4)).unwrap(),
            vec![node(1), node(2), node(4)]
        );
    }

    #[test]
    fn node_removal_removes_incident_links() {
        let mut graph = TopologyGraph::default();
        add_node(&mut graph, 1, true);
        add_node(&mut graph, 2, false);
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        graph.add_link(active_link(1, 1, 1, 2)).unwrap();
        graph.remove_node(node(2));
        assert_eq!(graph.links().count(), 0);
    }

    // CR-F3 regressions (docs/CODE_REVIEW_REMEDIATION_TODO.md, GRAPH-1..GRAPH-4). The first four
    // failed on baseline 4ca5042.

    fn members_of(graph: &TopologyGraph, id: u8) -> Vec<NodeId> {
        graph
            .segments()
            .find(|(segment_id, _)| **segment_id == segment(id))
            .map(|(_, segment)| segment.members().copied().collect())
            .unwrap_or_default()
    }

    fn star_with_one_client() -> TopologyGraph {
        let mut graph = TopologyGraph::default();
        add_node(&mut graph, 1, true);
        add_node(&mut graph, 2, false);
        add_node(&mut graph, 3, false);
        add_node(&mut graph, 4, true);
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        graph.add_link(active_link(1, 1, 1, 2)).unwrap();
        graph
    }

    #[test]
    fn cr_f3_identical_segment_reinsertion_keeps_attached_members() {
        let mut graph = star_with_one_client();
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        assert_eq!(members_of(&graph, 1), vec![node(1), node(2)]);
    }

    #[test]
    fn cr_f3_segment_nap_change_under_existing_links_is_rejected_atomically() {
        let mut graph = star_with_one_client();
        let before = graph.clone();
        assert!(
            graph
                .add_segment(PanSegment::new(segment(1), node(4)))
                .is_err()
        );
        assert_eq!(graph, before);
    }

    #[test]
    fn cr_f3_link_identity_rebinding_is_rejected_without_stale_membership() {
        let mut graph = star_with_one_client();
        let before = graph.clone();
        // Same LinkId, different PANU: must not silently rebind and leave node 2 a member.
        assert!(graph.add_link(active_link(1, 1, 1, 3)).is_err());
        assert_eq!(graph, before);
    }

    #[test]
    fn cr_f3_every_link_matches_its_segment_after_mutations() {
        let mut graph = star_with_one_client();
        // Whatever these return, the graph must stay internally consistent.
        let _ = graph.add_segment(PanSegment::new(segment(1), node(4)));
        let _ = graph.add_link(active_link(1, 1, 1, 3));
        for (_, link) in graph.links() {
            let (_, segment) = graph
                .segments()
                .find(|(id, _)| **id == link.segment_id)
                .expect("every link references an existing segment");
            assert_eq!(segment.nap, link.nap, "link NAP must match its segment NAP");
            assert!(segment.members().any(|member| *member == link.panu));
        }
        for (_, segment) in graph.segments() {
            for member in segment.members() {
                assert!(
                    *member == segment.nap
                        || graph
                            .links()
                            .any(|(_, link)| link.segment_id == segment.id && link.panu == *member),
                    "no stale member without a link"
                );
            }
        }
    }

    /// GRAPH-3: every link's endpoints exist, its segment exists with the same NAP, and its PANU
    /// is a member; every member is the NAP, an explicit member, or the PANU of a link there.
    fn assert_consistent(graph: &TopologyGraph) {
        for (_, link) in graph.links() {
            assert!(graph.node(&link.nap).is_some() && graph.node(&link.panu).is_some());
            let (_, segment) = graph
                .segments()
                .find(|(id, _)| **id == link.segment_id)
                .expect("every link references an existing segment");
            assert_eq!(segment.nap, link.nap);
            assert!(segment.members().any(|member| *member == link.panu));
        }
        for (_, segment) in graph.segments() {
            for member in segment.members() {
                assert!(
                    graph.node(member).is_some(),
                    "member {member:?} is not a node"
                );
                assert!(
                    segment.keeps(member)
                        || graph
                            .links()
                            .any(|(_, link)| link.segment_id == segment.id && link.panu == *member),
                    "stale member {member:?}"
                );
            }
        }
    }

    #[test]
    fn cr_f3_a_property_update_keeps_identity_and_membership() {
        let mut graph = star_with_one_client();
        let mut failed = active_link(1, 1, 1, 2);
        failed.health = LinkHealth::Failed;
        failed.cost = 9;
        assert_eq!(graph.add_link(failed), Ok(false));
        let (_, stored) = graph.links().next().unwrap();
        assert_eq!((stored.health, stored.cost), (LinkHealth::Failed, 9));
        assert_eq!(members_of(&graph, 1), vec![node(1), node(2)]);
        assert_consistent(&graph);
        // Downstream: a failed link no longer carries traffic, and recovers in place.
        assert_eq!(graph.shortest_path(node(1), node(2)), None);
        assert_eq!(graph.add_link(active_link(1, 1, 1, 2)), Ok(false));
        assert_eq!(
            graph.shortest_path(node(1), node(2)),
            Some(vec![node(1), node(2)])
        );
    }

    #[test]
    fn cr_f3_a_link_cannot_move_to_another_segment_or_nap() {
        let mut graph = star_with_one_client();
        graph
            .add_segment(PanSegment::new(segment(4), node(4)))
            .unwrap();
        let before = graph.clone();
        // Same LinkId, another segment with its own (matching) NAP.
        assert!(graph.add_link(active_link(1, 4, 4, 2)).is_err());
        // Same LinkId, swapped roles.
        assert!(graph.add_link(active_link(1, 1, 2, 1)).is_err());
        // Invalid in themselves: unknown segment, unknown endpoint, NAP not the segment's.
        assert!(graph.add_link(active_link(1, 9, 1, 2)).is_err());
        assert!(graph.add_link(active_link(1, 1, 1, 9)).is_err());
        assert!(graph.add_link(active_link(5, 1, 4, 2)).is_err());
        assert_eq!(graph, before);
        assert_consistent(&graph);
    }

    #[test]
    fn cr_f3_a_member_with_two_links_stays_until_the_last_is_removed() {
        let mut graph = star_with_one_client();
        graph.add_link(active_link(2, 1, 1, 2)).unwrap();
        assert_eq!(members_of(&graph, 1), vec![node(1), node(2)]);
        graph.remove_link(&link(1)).unwrap();
        assert_eq!(members_of(&graph, 1), vec![node(1), node(2)]);
        assert_consistent(&graph);
        graph.remove_link(&link(2)).unwrap();
        assert_eq!(members_of(&graph, 1), vec![node(1)]);
        assert_consistent(&graph);
    }

    #[test]
    fn cr_f3_removal_after_a_property_update_cleans_membership() {
        let mut graph = star_with_one_client();
        let mut degraded = active_link(1, 1, 1, 2);
        degraded.health = LinkHealth::Degraded;
        graph.add_link(degraded).unwrap();
        graph.remove_link(&link(1)).unwrap();
        assert_eq!(members_of(&graph, 1), vec![node(1)]);
        assert_consistent(&graph);
    }

    #[test]
    fn cr_f3_explicit_members_are_known_nodes_and_survive_link_changes() {
        let mut graph = star_with_one_client();
        // An explicit member that is not a node is refused, atomically.
        let mut unknown = PanSegment::new(segment(1), node(1));
        unknown.add_member(node(9));
        let before = graph.clone();
        assert!(graph.add_segment(unknown).is_err());
        assert_eq!(graph, before);

        // Reinsertion may add an explicit member (node 2, which also has a link) …
        let mut with_member = PanSegment::new(segment(1), node(1));
        with_member.add_member(node(2));
        assert_eq!(graph.add_segment(with_member), Ok(false));
        // … which stays when its link goes, until the node itself goes.
        graph.remove_link(&link(1)).unwrap();
        assert_eq!(members_of(&graph, 1), vec![node(1), node(2)]);
        assert_consistent(&graph);
        graph.remove_node(node(2));
        assert_eq!(members_of(&graph, 1), vec![node(1)]);
        assert_consistent(&graph);
    }

    #[test]
    fn cr_f3_duplicate_inserts_report_whether_anything_was_new() {
        let mut graph = star_with_one_client();
        assert_eq!(
            graph.add_segment(PanSegment::new(segment(1), node(1))),
            Ok(false)
        );
        assert_eq!(graph.add_link(active_link(1, 1, 1, 2)), Ok(false));
        assert_eq!(graph.add_link(active_link(2, 1, 1, 3)), Ok(true));
        assert_eq!(
            graph.add_segment(PanSegment::new(segment(4), node(4))),
            Ok(true)
        );
        assert_eq!(graph.links().count(), 2);
        assert_consistent(&graph);
    }

    #[test]
    fn cr_f3_refused_changes_leave_paths_deterministic() {
        // A two-segment route 2 – 1 – 4 – 3, as the routed tests build it.
        let mut graph = star_with_one_client();
        graph
            .add_segment(PanSegment::new(segment(4), node(4)))
            .unwrap();
        graph.add_link(active_link(2, 1, 1, 4)).unwrap();
        graph.add_link(active_link(3, 4, 4, 3)).unwrap();
        let path = graph.shortest_path(node(2), node(3));
        assert_eq!(path, Some(vec![node(2), node(1), node(4), node(3)]));
        // Refused rebinding and NAP change do not disturb routing.
        assert!(graph.add_link(active_link(3, 4, 4, 2)).is_err());
        assert!(
            graph
                .add_segment(PanSegment::new(segment(4), node(1)))
                .is_err()
        );
        assert_eq!(graph.shortest_path(node(2), node(3)), path);
        assert_consistent(&graph);
    }

    // FU-03 regression (docs/CODE_REVIEW_FOLLOWUP_TODO.md, REV-03, SEG-2/4). It states the
    // corrected contract and fails on 6d200d0, where a segment cloned from another graph keeps
    // that graph's link-derived members.
    #[test]
    #[ignore = "FU-03 regression: fails on 6d200d0 until FU-03 is fixed"]
    fn fu_03_a_segment_cloned_from_another_graph_brings_no_phantom_members() {
        let source = star_with_one_client(); // segment 1: NAP 1, client 2 via link 1
        let (_, cloned) = source
            .segments()
            .find(|(id, _)| **id == segment(1))
            .unwrap();
        let cloned = cloned.clone();

        // (a) The client does not exist in the destination graph.
        let mut missing = TopologyGraph::default();
        add_node(&mut missing, 1, true);
        if missing.add_segment(cloned.clone()).is_ok() {
            assert_consistent(&missing);
        }

        // (b) The client exists, but no link there justifies its membership.
        let mut unlinked = TopologyGraph::default();
        add_node(&mut unlinked, 1, true);
        add_node(&mut unlinked, 2, false);
        if unlinked.add_segment(cloned).is_ok() {
            assert_consistent(&unlinked);
            assert_eq!(members_of(&unlinked, 1), vec![node(1)]);
        }
    }
}

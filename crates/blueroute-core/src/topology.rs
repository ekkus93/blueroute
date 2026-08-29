use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{CoreError, ErrorKind, LinkId, NodeCapabilities, NodeId, SegmentId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkState {
    Planned,
    Connecting,
    Active,
    Disconnected,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkHealth {
    Unknown,
    Healthy,
    Degraded,
    Failed,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanSegment {
    pub id: SegmentId,
    pub nap: NodeId,
    members: BTreeSet<NodeId>,
}

impl PanSegment {
    pub fn new(id: SegmentId, nap: NodeId) -> Self {
        let mut members = BTreeSet::new();
        members.insert(nap);
        Self { id, nap, members }
    }

    pub fn add_member(&mut self, node: NodeId) -> bool {
        self.members.insert(node)
    }

    pub fn members(&self) -> impl Iterator<Item = &NodeId> {
        self.members.iter()
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
        }
        Some(removed)
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeCapabilities> {
        self.nodes.get(id)
    }

    pub fn add_segment(&mut self, segment: PanSegment) -> Result<bool, CoreError> {
        if !self.nodes.contains_key(&segment.nap) {
            return Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN segment NAP is not present in the topology",
            ));
        }
        Ok(self.segments.insert(segment.id, segment).is_none())
    }

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
        segment.add_member(link.panu);
        Ok(self.links.insert(link.id, link).is_none())
    }

    pub fn remove_link(&mut self, id: &LinkId) -> Option<PanLink> {
        let removed = self.links.remove(id)?;
        if let Some(segment) = self.segments.get_mut(&removed.segment_id) {
            let still_attached = self.links.values().any(|link| {
                link.segment_id == removed.segment_id && link.panu == removed.panu
            });
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

        let mut queue = VecDeque::from([start]);
        let mut previous = BTreeMap::<NodeId, NodeId>::new();
        let mut visited = BTreeSet::from([start]);

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

    pub fn is_routed_reachable(&self, start: NodeId, end: NodeId) -> bool {
        self.shortest_path(start, end)
            .is_some_and(|path| path.len() > 2)
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
        assert!(graph.is_routed_reachable(node(2), node(3)));
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
}

//! Pure desired-state decision for IPv4 forwarding between distinct PAN segments.
//! This does not touch sysctls, authorize peers, or claim physical Bluetooth
//! forwarding. The caller must supply locally authenticated active topology,
//! and use the Linux backend's ownership-aware forwarding lease for changes.

use std::collections::BTreeSet;

use crate::{CoreError, ErrorKind, NodeId, TopologyGraph};

/// Forwarding is needed only for a routing-capable node with live links in
/// at least two distinct PAN segments. An ordinary single-star NAP does not
/// need global IPv4 forwarding to bridge clients at layer 2.
///
/// Unknown routing capability is a denial. Untrusted advertisements must
/// never be promoted into this graph by the caller.
pub fn desired_ipv4_forwarding(graph: &TopologyGraph, local: NodeId) -> Result<bool, CoreError> {
    let capabilities = graph
        .node(&local)
        .ok_or_else(|| CoreError::new(ErrorKind::InvalidInput, "local node is not in topology"))?;
    if capabilities.can_route() != Some(true) {
        return Ok(false);
    }
    let active_segments: BTreeSet<_> = graph
        .links()
        .filter(|(_, link)| {
            link.is_forwarding_candidate() && (link.nap == local || link.panu == local)
        })
        .map(|(_, link)| link.segment_id)
        .collect();
    Ok(active_segments.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CapabilitySource, LinkHealth, LinkId, LinkState, NodeCapabilities, PanLink, PanSegment,
        SegmentId, Sourced,
    };

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn segment(value: u8) -> SegmentId {
        SegmentId::from_bytes([value; 16])
    }

    fn build_graph(router: Option<bool>) -> TopologyGraph {
        let mut graph = TopologyGraph::default();
        for value in 1..=4 {
            graph.add_node(
                node(value),
                NodeCapabilities {
                    routing: if value == 2 {
                        router.map(|value| Sourced::new(value, CapabilitySource::Configured))
                    } else {
                        None
                    },
                    ..NodeCapabilities::default()
                },
            );
        }
        graph
            .add_segment(PanSegment::new(segment(1), node(1)))
            .unwrap();
        graph
            .add_segment(PanSegment::new(segment(2), node(2)))
            .unwrap();
        graph
    }

    fn attach(
        graph: &mut TopologyGraph,
        id: u8,
        seg: u8,
        nap: u8,
        panu: u8,
        state: LinkState,
        health: LinkHealth,
    ) {
        graph
            .add_link(PanLink {
                id: LinkId::from_bytes([id; 16]),
                segment_id: segment(seg),
                nap: node(nap),
                panu: node(panu),
                state,
                health,
                cost: 1,
            })
            .unwrap();
    }

    #[test]
    fn single_star_never_enables_ip_forwarding() {
        let mut graph = build_graph(Some(true));
        attach(
            &mut graph,
            1,
            2,
            2,
            3,
            LinkState::Active,
            LinkHealth::Healthy,
        );
        attach(
            &mut graph,
            2,
            2,
            2,
            4,
            LinkState::Active,
            LinkHealth::Healthy,
        );
        assert!(!desired_ipv4_forwarding(&graph, node(2)).unwrap());
        assert!(!desired_ipv4_forwarding(&graph, node(3)).unwrap());
    }

    #[test]
    fn two_active_segments_require_explicit_router_capability() {
        let mut graph = build_graph(Some(true));
        attach(
            &mut graph,
            1,
            1,
            1,
            2,
            LinkState::Active,
            LinkHealth::Healthy,
        );
        attach(
            &mut graph,
            2,
            2,
            2,
            3,
            LinkState::Active,
            LinkHealth::Degraded,
        );
        assert!(desired_ipv4_forwarding(&graph, node(2)).unwrap());
        assert!(!desired_ipv4_forwarding(&graph, node(1)).unwrap());

        for router in [None, Some(false)] {
            let mut denied = build_graph(router);
            attach(
                &mut denied,
                1,
                1,
                1,
                2,
                LinkState::Active,
                LinkHealth::Healthy,
            );
            attach(
                &mut denied,
                2,
                2,
                2,
                3,
                LinkState::Active,
                LinkHealth::Healthy,
            );
            assert!(!desired_ipv4_forwarding(&denied, node(2)).unwrap());
        }
    }

    #[test]
    fn failed_or_disconnected_links_release_desired_forwarding() {
        let mut graph = build_graph(Some(true));
        attach(
            &mut graph,
            1,
            1,
            1,
            2,
            LinkState::Active,
            LinkHealth::Healthy,
        );
        attach(
            &mut graph,
            2,
            2,
            2,
            3,
            LinkState::Disconnected,
            LinkHealth::Healthy,
        );
        assert!(!desired_ipv4_forwarding(&graph, node(2)).unwrap());
        graph.remove_link(&LinkId::from_bytes([2; 16]));
        attach(
            &mut graph,
            2,
            2,
            2,
            3,
            LinkState::Active,
            LinkHealth::Failed,
        );
        assert!(!desired_ipv4_forwarding(&graph, node(2)).unwrap());
        graph.remove_link(&LinkId::from_bytes([2; 16]));
        attach(
            &mut graph,
            2,
            2,
            2,
            3,
            LinkState::Active,
            LinkHealth::Healthy,
        );
        assert!(desired_ipv4_forwarding(&graph, node(2)).unwrap());
        assert!(desired_ipv4_forwarding(&graph, node(4)).is_ok());
        assert_eq!(
            desired_ipv4_forwarding(&graph, node(9)).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }
}

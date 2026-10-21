//! Reconcile authenticated, locally observed PAN topology with Linux IPv4 forwarding.
//!
//! This is a software integration boundary. It does not authenticate topology
//! claims, install routes, or establish physical Bluetooth connectivity.

use blueroute_core::{CoreError, ErrorKind, NodeId, TopologyGraph, desired_ipv4_forwarding};

use crate::IpNetworkBackend;

/// Apply the forwarding decision to an ownership-aware Linux network backend.
///
/// The caller must supply the complete, locally verified set of BlueRoute
/// network topologies on this host, not a single network or untrusted peer
/// advertisements. Forwarding is host-global: a loss in one network must not
/// disable forwarding still required by another. An empty complete snapshot
/// releases BlueRoute's lease. A nonempty snapshot missing the local node in
/// every topology is invalid and never changes the backend.
///
/// The backend handles idempotence and preserves pre-existing host forwarding
/// policy via its owned lease. Reconcile after topology changes and restart to
/// repair observed drift.
pub async fn reconcile_ipv4_forwarding(
    backend: &impl IpNetworkBackend,
    topologies: &[TopologyGraph],
    local: NodeId,
) -> Result<bool, CoreError> {
    let mut saw_local = false;
    let mut desired = false;
    for topology in topologies {
        if topology.node(&local).is_some() {
            saw_local = true;
            desired |= desired_ipv4_forwarding(topology, local)?;
        }
    }
    if !topologies.is_empty() && !saw_local {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "local node absent from all supplied BlueRoute network topologies",
        ));
    }
    backend.set_ipv4_forwarding(desired).await?;
    Ok(desired)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use blueroute_core::{
        CapabilitySource, ErrorKind, LinkHealth, LinkId, LinkState, NodeCapabilities, PanLink,
        PanSegment, SegmentId, Sourced,
    };
    use futures_lite::future::block_on;

    use super::*;
    use crate::{BackendFuture, InterfaceAddress, LinuxRoute};

    #[derive(Default)]
    struct FakeBackend {
        applied: Mutex<Vec<bool>>,
        fail: bool,
    }

    impl IpNetworkBackend for FakeBackend {
        fn addresses(&self) -> BackendFuture<'_, Vec<InterfaceAddress>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn ensure_address(&self, _address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn remove_address(&self, _address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn routes(&self) -> BackendFuture<'_, Vec<LinuxRoute>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn ensure_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn remove_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn set_ipv4_forwarding(&self, enabled: bool) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                if self.fail {
                    return Err(CoreError::new(
                        ErrorKind::NetworkBackendUnavailable,
                        "simulated forwarding backend failure",
                    ));
                }
                self.applied.lock().unwrap().push(enabled);
                Ok(())
            })
        }
    }

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn segment(value: u8) -> SegmentId {
        SegmentId::from_bytes([value; 16])
    }

    fn graph_with_two_active_segments() -> TopologyGraph {
        let mut graph = TopologyGraph::default();
        for value in 1..=3 {
            graph.add_node(
                node(value),
                NodeCapabilities {
                    routing: if value == 2 {
                        Some(Sourced::new(true, CapabilitySource::Configured))
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
        for (id, pan, nap, panu) in [(1, 1, 1, 2), (2, 2, 2, 3)] {
            graph
                .add_link(PanLink {
                    id: LinkId::from_bytes([id; 16]),
                    segment_id: segment(pan),
                    nap: node(nap),
                    panu: node(panu),
                    state: LinkState::Active,
                    health: LinkHealth::Healthy,
                    cost: 1,
                })
                .unwrap();
        }
        graph
    }

    #[test]
    fn two_segment_router_enables_and_link_loss_releases_forwarding() {
        let backend = FakeBackend::default();
        let mut graph = graph_with_two_active_segments();

        assert!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                std::slice::from_ref(&graph),
                node(2)
            ))
            .unwrap()
        );
        graph.remove_link(&LinkId::from_bytes([2; 16]));
        assert!(
            !block_on(reconcile_ipv4_forwarding(
                &backend,
                std::slice::from_ref(&graph),
                node(2)
            ))
            .unwrap()
        );
        assert_eq!(*backend.applied.lock().unwrap(), [true, false]);
    }

    #[test]
    fn host_wide_aggregation_preserves_other_networks_and_releases_last_lease() {
        let backend = FakeBackend::default();
        let active = graph_with_two_active_segments();
        let mut inactive = active.clone();
        inactive.remove_link(&LinkId::from_bytes([2; 16]));

        assert!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                &[inactive.clone(), active.clone()],
                node(2),
            ))
            .unwrap()
        );
        assert!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                &[active, inactive.clone()],
                node(2),
            ))
            .unwrap()
        );
        assert!(
            !block_on(reconcile_ipv4_forwarding(
                &backend,
                std::slice::from_ref(&inactive),
                node(2),
            ))
            .unwrap()
        );
        assert!(!block_on(reconcile_ipv4_forwarding(&backend, &[], node(2))).unwrap());
        assert_eq!(*backend.applied.lock().unwrap(), [true, true, false, false]);
    }

    #[test]
    fn unknown_or_unapproved_local_node_cannot_enable_forwarding() {
        let backend = FakeBackend::default();
        let graph = graph_with_two_active_segments();
        assert_eq!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                std::slice::from_ref(&graph),
                node(4)
            ))
            .unwrap_err()
            .kind(),
            ErrorKind::InvalidInput
        );
        assert!(backend.applied.lock().unwrap().is_empty());

        assert!(
            !block_on(reconcile_ipv4_forwarding(
                &backend,
                std::slice::from_ref(&graph),
                node(1)
            ))
            .unwrap()
        );
        assert_eq!(*backend.applied.lock().unwrap(), [false]);
    }

    #[test]
    fn backend_failure_is_not_reported_as_success() {
        let backend = FakeBackend {
            applied: Mutex::new(Vec::new()),
            fail: true,
        };
        let graph = graph_with_two_active_segments();
        assert_eq!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                std::slice::from_ref(&graph),
                node(2)
            ))
            .unwrap_err()
            .kind(),
            ErrorKind::NetworkBackendUnavailable
        );
        assert!(backend.applied.lock().unwrap().is_empty());
    }
}

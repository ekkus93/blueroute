//! FU-01 regressions (docs/CODE_REVIEW_FOLLOWUP_TODO.md, REV-01, PUB-1..4): a slow network-list
//! observation must not hold back the announcement of an unrelated, already published change.
//! On 6d200d0 the publisher samples the network list inline, so a pending read blocks every
//! signal until it finishes. Needs a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-daemon --test publisher_progress --locked -- --ignored`
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_core::{DisplayName, HealthLevel, NetworkId, NodeCapabilities, NodeId};
use blueroute_daemon::{
    DaemonService, NETWORK_RECHECK, NetworkOperationFuture, NetworkOperations,
    spawn_state_publisher,
};
use blueroute_protocol::{
    ApiVersion, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus,
    NetworkSummary, STATE_CHANGED_SIGNAL,
};
use futures_lite::{StreamExt, future};
use zbus::connection::Builder;
use zbus::{Connection, Proxy};

/// EVENT-6 / PUB-4: delivery bound after publication.
const DELIVERY_BOUND: Duration = Duration::from_secs(1);

/// A network list whose reads stay pending while `held` is set.
struct SlowNetworks {
    held: Arc<AtomicBool>,
}

impl NetworkOperations for SlowNetworks {
    fn create_network(&self, _name: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        Box::pin(async { Ok(NetworkId::from_bytes([0x5b; 16])) })
    }

    fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>> {
        Box::pin(async move {
            while self.held.load(Ordering::SeqCst) {
                Timer::after(Duration::from_millis(5)).await;
            }
            Ok(Vec::new())
        })
    }

    fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// Releases the held read when dropped, so the publisher can always be shut down afterwards.
struct ReleaseOnDrop(Arc<AtomicBool>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[test]
#[ignore = "FU-01 regression: fails on 6d200d0 until FU-01 is fixed; needs dbus-run-session"]
fn fu_01_a_pending_network_read_does_not_hold_back_a_health_change() -> Result<(), Box<dyn Error>> {
    future::block_on(async {
        let held = Arc::new(AtomicBool::new(false));
        let service = DaemonService::with_network_operations(
            DaemonStatus {
                api_version: ApiVersion::CURRENT,
                local_node: Some(NodeId::from_bytes([0xf0; 16])),
                current_network: None,
                health: HealthLevel::Healthy,
                capabilities: NodeCapabilities::default(),
            },
            Arc::new(SlowNetworks { held: held.clone() }),
        );
        let connection = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, service.clone())?
            .build()
            .await?;
        let _publisher = spawn_state_publisher(connection.clone(), service.clone())?;
        // Declared after the publisher, so it is dropped first and the publisher can stop.
        let _release = ReleaseOnDrop(held.clone());

        let observer = Connection::session().await?;
        let proxy = Proxy::new(
            &observer,
            DBUS_SERVICE_NAME,
            DBUS_OBJECT_PATH,
            DBUS_INTERFACE_NAME,
        )
        .await?;
        let mut signals = proxy.receive_signal(STATE_CHANGED_SIGNAL).await?;

        // The network backend stalls; let the publisher's periodic network sample reach it.
        held.store(true, Ordering::SeqCst);
        Timer::after(NETWORK_RECHECK + Duration::from_millis(300)).await;

        // A real worker publication, unrelated to the network list.
        let published = Instant::now();
        assert!(service.update_runtime_observation(
            HealthLevel::Degraded,
            NodeCapabilities::default(),
            None
        )?);
        let delivered = future::or(async { signals.next().await.is_some() }, async {
            Timer::after(DELIVERY_BOUND).await;
            false
        })
        .await;
        assert!(
            delivered,
            "no StateChanged within {DELIVERY_BOUND:?} of a published health change while \
             the network read was pending"
        );
        eprintln!("delivered in {:?}", published.elapsed());
        Ok(())
    })
}

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

/// A network list whose reads stay pending while `held` is set and fail while `failing` is.
#[derive(Default)]
struct SlowNetworks {
    held: Arc<AtomicBool>,
    failing: Arc<AtomicBool>,
    list: Arc<std::sync::Mutex<Vec<NetworkSummary>>>,
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
            if self.failing.load(Ordering::SeqCst) {
                return Err(blueroute_core::CoreError::new(
                    blueroute_core::ErrorKind::PersistenceError,
                    "membership store unreadable",
                ));
            }
            Ok(self.list.lock().unwrap().clone())
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
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_01_a_pending_network_read_does_not_hold_back_a_health_change() -> Result<(), Box<dyn Error>> {
    let _bus = lock();
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
            Arc::new(SlowNetworks {
                held: held.clone(),
                ..SlowNetworks::default()
            }),
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

/// The tests share the daemon's well-known name on one private bus.
static BUS: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    service: DaemonService,
    held: Arc<AtomicBool>,
    failing: Arc<AtomicBool>,
    list: Arc<std::sync::Mutex<Vec<NetworkSummary>>>,
    publisher: Option<blueroute_daemon::StatePublisherWorker>,
    signals: zbus::proxy::SignalStream<'static>,
    _connection: Connection,
    _observer: Connection,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.held.store(false, Ordering::SeqCst);
    }
}

/// A served service and its production publisher; `held` decides whether the network backend
/// is already stalled when the publisher starts.
async fn fixture(held: bool) -> Result<Fixture, Box<dyn Error>> {
    let networks = SlowNetworks::default();
    networks.held.store(held, Ordering::SeqCst);
    let (held, failing, list) = (
        networks.held.clone(),
        networks.failing.clone(),
        networks.list.clone(),
    );
    let service = DaemonService::with_network_operations(
        DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([0xf2; 16])),
            current_network: None,
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        },
        Arc::new(networks),
    );
    let connection = Builder::session()?
        .name(DBUS_SERVICE_NAME)?
        .serve_at(DBUS_OBJECT_PATH, service.clone())?
        .build()
        .await?;
    let observer = Connection::session().await?;
    let proxy = zbus::Proxy::new_owned(
        observer.clone(),
        DBUS_SERVICE_NAME,
        DBUS_OBJECT_PATH,
        DBUS_INTERFACE_NAME,
    )
    .await?;
    let signals = proxy.receive_signal(STATE_CHANGED_SIGNAL).await?;
    let publisher = spawn_state_publisher(connection.clone(), service.clone())?;
    Ok(Fixture {
        service,
        held,
        failing,
        list,
        publisher: Some(publisher),
        signals,
        _connection: connection,
        _observer: observer,
    })
}

impl Fixture {
    async fn signal_within(&mut self, bound: Duration) -> bool {
        let signals = &mut self.signals;
        future::or(async { signals.next().await.is_some() }, async {
            Timer::after(bound).await;
            false
        })
        .await
    }

    /// Signals arriving before nothing more comes for 700 ms.
    async fn drain(&mut self) -> usize {
        let mut count = 0;
        while self.signal_within(Duration::from_millis(700)).await {
            count += 1;
        }
        count
    }

    fn publish_health(&self, level: HealthLevel) {
        self.service
            .update_runtime_observation(level, NodeCapabilities::default(), None)
            .unwrap();
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    BUS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_01_a_read_pending_from_startup_does_not_block_announcements() {
    let _bus = lock();
    future::block_on(async {
        let mut fx = fixture(true).await.unwrap();
        // Let the emitter take its baseline first: a change racing that baseline is GAP-01, which
        // FU-05 covers separately. Here only the never-finishing initial sample is in the way.
        Timer::after(Duration::from_millis(300)).await;
        fx.publish_health(HealthLevel::Degraded);
        assert!(
            fx.signal_within(DELIVERY_BOUND).await,
            "blocked by the initial sample"
        );
    });
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_01_shutdown_finishes_while_a_sample_is_pending() {
    let _bus = lock();
    future::block_on(async {
        let mut fx = fixture(true).await.unwrap();
        Timer::after(Duration::from_millis(200)).await;
        let stopping = Instant::now();
        drop(fx.publisher.take()); // the read is still held
        let took = stopping.elapsed();
        assert!(took < Duration::from_secs(2), "shutdown took {took:?}");
        eprintln!("shutdown with a pending sample took {took:?}");
    });
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_01_a_released_read_brings_the_final_network_state() {
    let _bus = lock();
    future::block_on(async {
        let mut fx = fixture(false).await.unwrap();
        Timer::after(Duration::from_millis(300)).await;
        fx.drain().await;
        fx.held.store(true, Ordering::SeqCst);
        Timer::after(NETWORK_RECHECK + Duration::from_millis(300)).await;
        // The list changes while the read is stalled; once released, observers must hear of it.
        fx.list.lock().unwrap().push(NetworkSummary {
            id: NetworkId::from_bytes([0x5c; 16]),
            name: DisplayName::new("Late").unwrap(),
            member_count: 1,
        });
        fx.held.store(false, Ordering::SeqCst);
        assert!(
            fx.signal_within(NETWORK_RECHECK + DELIVERY_BOUND).await,
            "the released sample's change was not announced"
        );
    });
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_01_a_failing_sample_is_announced_once_and_its_recovery_too() {
    let _bus = lock();
    future::block_on(async {
        let mut fx = fixture(false).await.unwrap();
        Timer::after(Duration::from_millis(300)).await;
        fx.drain().await;
        // Starting to fail is a change (not an empty list); failing on is not.
        fx.failing.store(true, Ordering::SeqCst);
        assert!(fx.signal_within(NETWORK_RECHECK + DELIVERY_BOUND).await);
        Timer::after(NETWORK_RECHECK * 2).await;
        assert_eq!(fx.drain().await, 0, "a persistently failing sample looped");
        fx.failing.store(false, Ordering::SeqCst);
        assert!(fx.signal_within(NETWORK_RECHECK + DELIVERY_BOUND).await);
        // While failing, other changes still went out promptly.
        fx.failing.store(true, Ordering::SeqCst);
        fx.publish_health(HealthLevel::Error);
        assert!(fx.signal_within(DELIVERY_BOUND).await);
    });
}

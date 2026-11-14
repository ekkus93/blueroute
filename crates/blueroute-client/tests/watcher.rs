//! CR-F2 watcher regressions (WATCH-1..WATCH-4) for [`blueroute_client::watch`], the loop the
//! desktop and the TUI both run. A private bus stands in for the system bus and the backoff sleep
//! is injected (the requested delay is recorded, the real wait is a few milliseconds), so the
//! production 1 s → 30 s schedule is verified without long sleeps. Run with:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test watcher --locked -- --ignored`
use std::error::Error;
use std::future::Future;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_client::{BlueRouteClient, ClientError, WatchSignal, watch};
use blueroute_core::HealthLevel;
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event, encode_event};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, fdo, interface};

/// The tests share the daemon's well-known name on one private bus, so they run one at a time.
static BUS: Mutex<()> = Mutex::new(());

/// WATCH-4: owner loss must reach the watcher within this bound without any Event.
const LOSS_BOUND: Duration = Duration::from_secs(2);
const COMPATIBLE: (u16, u16) = (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor);
const INCOMPATIBLE: (u16, u16) = (99, 0);

struct Daemon {
    version: (u16, u16),
}

#[interface(name = "org.blueroute.Service1")]
impl Daemon {
    fn version(&self) -> (u16, u16) {
        self.version
    }

    fn request(&self, _payload: &str) -> fdo::Result<String> {
        Ok(r#"{"type":"ack"}"#.to_owned())
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;
}

async fn daemon(version: (u16, u16), owns_name: bool) -> Result<Connection, Box<dyn Error>> {
    let mut builder = Builder::session()?;
    if owns_name {
        builder = builder.name(DBUS_SERVICE_NAME)?;
    }
    Ok(builder
        .serve_at(DBUS_OBJECT_PATH, Daemon { version })?
        .build()
        .await?)
}

async fn emit(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let interface = connection
        .object_server()
        .interface::<_, Daemon>(DBUS_OBJECT_PATH)
        .await?;
    let payload = encode_event(&Event::HealthChanged(HealthLevel::Degraded))?;
    Daemon::event(interface.signal_emitter(), &payload).await?;
    Ok(())
}

async fn stop(connection: Connection) -> Result<(), Box<dyn Error>> {
    connection.release_name(DBUS_SERVICE_NAME).await?;
    connection.close().await?;
    Ok(())
}

/// A fresh client on its own private-bus connection, as each production attempt makes one.
async fn connect() -> Result<BlueRouteClient, ClientError> {
    let connection = Connection::session().await.map_err(ClientError::Bus)?;
    BlueRouteClient::from_connection(connection).await
}

/// What the watcher under test has reported, and the backoff delays it asked for.
#[derive(Clone, Default)]
struct Log {
    signals: Arc<Mutex<Vec<WatchSignal>>>,
    delays: Arc<Mutex<Vec<Duration>>>,
}

impl Log {
    fn signals(&self) -> Vec<WatchSignal> {
        self.signals.lock().unwrap().clone()
    }

    fn delays(&self) -> Vec<Duration> {
        self.delays.lock().unwrap().clone()
    }

    /// Wait up to `bound` for `done`; the elapsed time on success.
    async fn until(&self, bound: Duration, done: impl Fn(&Log) -> bool) -> Option<Duration> {
        let start = Instant::now();
        while start.elapsed() < bound {
            if done(self) {
                return Some(start.elapsed());
            }
            Timer::after(Duration::from_millis(5)).await;
        }
        done(self).then(|| start.elapsed())
    }

    fn count(&self, signal: WatchSignal) -> usize {
        self.signals()
            .iter()
            .filter(|seen| **seen == signal)
            .count()
    }
}

/// Run the production watcher on a private-bus client and a fast clock alongside `scenario`.
/// With `initial`, a daemon of that version holds the name before the watcher starts and is
/// handed to the scenario. The watcher is dropped (cancelled) when the scenario finishes.
fn run(
    initial: Option<(u16, u16)>,
    scenario: impl AsyncFnOnce(Log, Option<Connection>) -> Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let first = match initial {
            Some(version) => Some(daemon(version, true).await?),
            None => None,
        };
        let log = Log::default();
        let (signals, delays) = (log.signals.clone(), log.delays.clone());
        let watcher = watch(
            connect,
            move |delay| {
                delays.lock().unwrap().push(delay);
                Timer::after(Duration::from_millis(10)).map(|_| ())
            },
            move |signal| {
                signals.lock().unwrap().push(signal);
                ControlFlow::Continue(())
            },
        );
        future::or(
            async {
                watcher.await;
                Err::<(), Box<dyn Error>>("the watcher stopped on its own".into())
            },
            scenario(log, first),
        )
        .await
    })
}

trait MapUnit: Future + Sized {
    fn map(self, f: impl FnOnce(Self::Output)) -> impl Future<Output = ()> {
        async move { f(self.await) }
    }
}
impl<F: Future> MapUnit for F {}

use WatchSignal::{Changed, Connected, Disconnected};

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_idle_loss_is_reported_and_a_compatible_return_reconnects() -> Result<(), Box<dyn Error>> {
    run(Some(COMPATIBLE), async |log, a| {
        let a = a.ok_or("no initial daemon")?;
        log.until(LOSS_BOUND, |log| log.count(Connected) == 1)
            .await
            .ok_or("never connected")?;

        // The daemon stops without ever emitting an Event.
        let stopped_at = Instant::now();
        stop(a).await?;
        log.until(LOSS_BOUND, |log| log.count(Disconnected) == 1)
            .await
            .ok_or("idle owner loss not reported within 2s")?;
        let noticed = stopped_at.elapsed();
        assert!(noticed < LOSS_BOUND, "{noticed:?}");
        eprintln!("owner loss reached the watcher in {noticed:?}");

        // A compatible daemon returns: a fresh subscription, no event or manual refresh needed.
        let b = daemon(COMPATIBLE, true).await?;
        log.until(LOSS_BOUND, |log| log.count(Connected) == 2)
            .await
            .ok_or("did not reconnect to the returning daemon")?;
        emit(&b).await?;
        log.until(LOSS_BOUND, |log| log.count(Changed) == 1)
            .await
            .ok_or("the new subscription did not deliver")?;
        // Exactly one listener: one event, one Changed.
        Timer::after(Duration::from_millis(100)).await;
        assert_eq!(log.signals(), [Connected, Disconnected, Connected, Changed]);
        assert_eq!(
            log.delays()[0],
            Duration::from_secs(1),
            "{:?}",
            log.delays()
        );
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_direct_replacement_moves_the_watcher_to_the_new_owner() -> Result<(), Box<dyn Error>> {
    run(Some(COMPATIBLE), async |log, a| {
        let a = a.ok_or("no initial daemon")?;
        let b = daemon(COMPATIBLE, false).await?;
        log.until(LOSS_BOUND, |log| log.count(Connected) == 1)
            .await
            .ok_or("never connected")?;

        // A hands the name straight to B and stays connected.
        a.release_name(DBUS_SERVICE_NAME).await?;
        b.request_name(DBUS_SERVICE_NAME).await?;
        log.until(LOSS_BOUND, |log| log.count(Connected) == 2)
            .await
            .ok_or("did not move to the replacement")?;
        emit(&a).await?; // the retired owner is no longer heard
        emit(&b).await?;
        log.until(LOSS_BOUND, |log| log.count(Changed) == 1)
            .await
            .ok_or("the replacement's event was not delivered")?;
        Timer::after(Duration::from_millis(100)).await;
        assert_eq!(log.signals(), [Connected, Disconnected, Connected, Changed]);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_an_incompatible_return_stays_unavailable() -> Result<(), Box<dyn Error>> {
    run(Some(COMPATIBLE), async |log, a| {
        let a = a.ok_or("no initial daemon")?;
        log.until(LOSS_BOUND, |log| log.count(Connected) == 1)
            .await
            .ok_or("never connected")?;
        stop(a).await?;

        let wrong = daemon(INCOMPATIBLE, true).await?;
        let attempts = log.delays().len();
        log.until(LOSS_BOUND, |log| log.delays().len() >= attempts + 3)
            .await
            .ok_or("the watcher stopped retrying")?;
        emit(&wrong).await?;
        Timer::after(Duration::from_millis(100)).await;
        assert_eq!(
            log.signals(),
            [Connected, Disconnected],
            "incompatible owner was used"
        );

        stop(wrong).await?;
        let _right = daemon(COMPATIBLE, true).await?;
        log.until(LOSS_BOUND, |log| log.count(Connected) == 2)
            .await
            .ok_or("did not reconnect once a compatible daemon returned")?;
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_backoff_keeps_the_production_schedule_and_reports_loss_once() -> Result<(), Box<dyn Error>>
{
    run(None, async |log, _| {
        // No daemon at all.
        log.until(LOSS_BOUND, |log| log.delays().len() >= 8)
            .await
            .ok_or("the watcher stopped retrying")?;
        let secs: Vec<u64> = log.delays().iter().take(8).map(Duration::as_secs).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(log.signals(), [Disconnected]);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_the_watcher_stops_when_its_consumer_does() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let _daemon = daemon(COMPATIBLE, true).await?;
        let mut seen = Vec::new();
        let stopped = future::or(
            async {
                watch(
                    connect,
                    |_| Timer::after(Duration::from_millis(10)).map(|_| ()),
                    |signal| {
                        seen.push(signal);
                        // The consumer is gone (a dropped TUI receiver, an unmounted view).
                        ControlFlow::Break(())
                    },
                )
                .await;
                true
            },
            async {
                Timer::after(LOSS_BOUND).await;
                false
            },
        )
        .await;
        assert!(
            stopped,
            "the watcher kept running after its consumer stopped"
        );
        assert_eq!(seen, [Connected]);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_a_real_daemon_change_reaches_the_production_watcher() -> Result<(), Box<dyn Error>> {
    use blueroute_core::{NodeCapabilities, NodeId};
    use blueroute_daemon::{DaemonService, spawn_state_publisher};

    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        // The real service and publisher, as the daemon's main wires them.
        let service = DaemonService::new(
            NodeId::from_bytes([0xc1; 16]),
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        );
        let connection = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, service.clone())?
            .build()
            .await?;
        let _publisher = spawn_state_publisher(connection.clone(), service.clone())?;

        let log = Log::default();
        let signals = log.signals.clone();
        let watcher = watch(
            connect,
            |_| Timer::after(Duration::from_millis(10)).map(|_| ()),
            move |signal| {
                signals.lock().unwrap().push(signal);
                ControlFlow::Continue(())
            },
        );
        future::or(
            async {
                watcher.await;
                Err::<(), Box<dyn Error>>("the watcher stopped on its own".into())
            },
            async {
                log.until(LOSS_BOUND, |log| log.count(Connected) == 1)
                    .await
                    .ok_or("never connected")?;
                // Startup announcements (FU-05) may arrive first; count only what follows.
                Timer::after(Duration::from_millis(300)).await;
                let before = log.count(Changed);
                // A worker observation, with no typed Event and no manual refresh.
                let changed_at = Instant::now();
                service.update_runtime_observation(
                    HealthLevel::Degraded,
                    NodeCapabilities::default(),
                    None,
                )?;
                log.until(Duration::from_secs(1), |log| log.count(Changed) > before)
                    .await
                    .ok_or("the change did not reach the watcher within 1s")?;
                eprintln!(
                    "daemon change reached the watcher in {:?}",
                    changed_at.elapsed()
                );
                Ok(())
            },
        )
        .await
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_05_a_replacement_daemons_pre_baseline_change_reaches_the_watcher()
-> Result<(), Box<dyn Error>> {
    use blueroute_core::{NodeCapabilities, NodeId};
    use blueroute_daemon::{DaemonService, spawn_state_publisher};

    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let a = daemon(COMPATIBLE, true).await?;
        let log = Log::default();
        let signals = log.signals.clone();
        let watcher = watch(
            connect,
            |_| Timer::after(Duration::from_millis(10)).map(|_| ()),
            move |signal| {
                signals.lock().unwrap().push(signal);
                ControlFlow::Continue(())
            },
        );
        let scenario = async {
            log.until(LOSS_BOUND, |log| log.count(Connected) == 1)
                .await
                .ok_or("never connected to A")?;
            stop(a).await?;
            // B takes over; the watcher reconnects and rereads B before B's publisher exists.
            let service = DaemonService::new(
                NodeId::from_bytes([0xc5; 16]),
                HealthLevel::Healthy,
                NodeCapabilities::default(),
            );
            let b = Builder::session()?
                .name(DBUS_SERVICE_NAME)?
                .serve_at(DBUS_OBJECT_PATH, service.clone())?
                .build()
                .await?;
            log.until(LOSS_BOUND, |log| log.count(Connected) == 2)
                .await
                .ok_or("never connected to B")?;
            let before = log.count(Changed);
            // B changes before its publisher takes a baseline …
            service.update_runtime_observation(
                HealthLevel::Degraded,
                NodeCapabilities::default(),
                None,
            )?;
            let _publisher = spawn_state_publisher(b.clone(), service.clone())?;
            // … and the watcher still learns of it.
            log.until(Duration::from_secs(1), |log| log.count(Changed) > before)
                .await
                .ok_or("B's pre-baseline change never reached the watcher")?;
            Ok::<(), Box<dyn Error>>(())
        };
        future::or(
            async {
                watcher.await;
                Err::<(), Box<dyn Error>>("the watcher stopped on its own".into())
            },
            scenario,
        )
        .await
    })
}

/// A network list that stays pending while `held` is set.
struct StalledNetworks {
    held: Arc<std::sync::atomic::AtomicBool>,
}

impl blueroute_daemon::NetworkOperations for StalledNetworks {
    fn create_network(
        &self,
        _name: blueroute_core::DisplayName,
    ) -> blueroute_daemon::NetworkOperationFuture<'_, blueroute_core::NetworkId> {
        Box::pin(async { Ok(blueroute_core::NetworkId::from_bytes([0x5d; 16])) })
    }
    fn leave_network(&self) -> blueroute_daemon::NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn list_networks(
        &self,
    ) -> blueroute_daemon::NetworkOperationFuture<'_, Vec<blueroute_protocol::NetworkSummary>> {
        Box::pin(async move {
            while self.held.load(std::sync::atomic::Ordering::SeqCst) {
                Timer::after(Duration::from_millis(5)).await;
            }
            Ok(Vec::new())
        })
    }
    fn start_discovery(&self) -> blueroute_daemon::NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn stop_discovery(&self) -> blueroute_daemon::NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_integration_the_watcher_hears_real_changes_while_network_sampling_is_stalled()
-> Result<(), Box<dyn Error>> {
    use blueroute_core::{NodeCapabilities, NodeId};
    use blueroute_daemon::{DaemonService, spawn_state_publisher};
    use std::sync::atomic::{AtomicBool, Ordering};

    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        // Cold start with the network backend stalled from the outset, and a change made before
        // the publisher exists (FU-01 + FU-05 together).
        let held = Arc::new(AtomicBool::new(true));
        struct Release(Arc<AtomicBool>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let service = DaemonService::with_network_operations(
            blueroute_protocol::DaemonStatus {
                api_version: ApiVersion::CURRENT,
                local_node: Some(NodeId::from_bytes([0xc7; 16])),
                current_network: None,
                health: HealthLevel::Healthy,
                capabilities: NodeCapabilities::default(),
            },
            Arc::new(StalledNetworks { held: held.clone() }),
        );
        let connection = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, service.clone())?
            .build()
            .await?;
        let log = Log::default();
        let signals = log.signals.clone();
        let watcher = watch(
            connect,
            |_| Timer::after(Duration::from_millis(10)).map(|_| ()),
            move |signal| {
                signals.lock().unwrap().push(signal);
                ControlFlow::Continue(())
            },
        );
        let scenario = async {
            log.until(LOSS_BOUND, |log| log.count(Connected) == 1)
                .await
                .ok_or("never connected")?;
            // The watcher has read; now a change, then the publisher, with sampling stalled.
            let before = log.count(Changed);
            service.update_runtime_observation(
                HealthLevel::Degraded,
                NodeCapabilities::default(),
                None,
            )?;
            let publisher = spawn_state_publisher(connection.clone(), service.clone())?;
            let _release = Release(held.clone()); // dropped before the publisher
            log.until(Duration::from_secs(1), |log| log.count(Changed) > before)
                .await
                .ok_or("cold-start change not announced while sampling was stalled")?;
            // A later change is announced promptly too, still with sampling stalled.
            Timer::after(Duration::from_millis(300)).await;
            let before = log.count(Changed);
            service.update_runtime_observation(
                HealthLevel::Error,
                NodeCapabilities::default(),
                None,
            )?;
            log.until(Duration::from_secs(1), |log| log.count(Changed) > before)
                .await
                .ok_or("a change was held back by the stalled network sample")?;
            drop(_release);
            drop(publisher);
            Ok::<(), Box<dyn Error>>(())
        };
        future::or(
            async {
                watcher.await;
                Err::<(), Box<dyn Error>>("the watcher stopped on its own".into())
            },
            scenario,
        )
        .await
    })
}

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

//! FU-02 regressions (docs/CODE_REVIEW_FOLLOWUP_TODO.md, REV-02, OWNER-1..4): the owner of an
//! event must stay monitored while that event's version is checked. On 6d200d0 an owner that
//! hands its name over inside that check still delivers, or delays the loss indefinitely.
//! Needs a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test event_owner_race --locked -- --ignored`
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_io::Timer;
use blueroute_client::BlueRouteClient;
use blueroute_core::HealthLevel;
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event, encode_event};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, fdo, interface};

/// The tests share the daemon's well-known name on one private bus.
static BUS: Mutex<()> = Mutex::new(());

const COMPATIBLE: (u16, u16) = (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor);

/// A daemon whose `handover`-th Version call gives the name to `successor` and, with `hold`,
/// then waits for `release` before replying.
struct Fake {
    version: (u16, u16),
    handover: Option<usize>,
    hold: bool,
    version_calls: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    own: Arc<OnceLock<Connection>>,
    successor: Arc<OnceLock<Connection>>,
    release: Arc<AtomicBool>,
}

impl Fake {
    fn new(handover: Option<usize>, hold: bool) -> Self {
        Self {
            version: COMPATIBLE,
            handover,
            hold,
            version_calls: Arc::default(),
            requests: Arc::default(),
            own: Arc::default(),
            successor: Arc::default(),
            release: Arc::default(),
        }
    }
}

#[interface(name = "org.blueroute.Service1")]
impl Fake {
    async fn version(&self) -> (u16, u16) {
        let call = self.version_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.handover == Some(call) {
            let own = self.own.get().expect("own connection is set");
            own.release_name(DBUS_SERVICE_NAME).await.expect("release");
            // Without a successor the name simply has no owner any more.
            if let Some(successor) = self.successor.get() {
                successor
                    .request_name(DBUS_SERVICE_NAME)
                    .await
                    .expect("successor takes the name");
            }
            while self.hold && !self.release.load(Ordering::SeqCst) {
                Timer::after(Duration::from_millis(5)).await;
            }
        }
        self.version
    }

    async fn request(&self, _payload: &str) -> fdo::Result<String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(r#"{"type":"ack"}"#.to_owned())
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;
}

async fn serve(fake: Fake, owns_name: bool) -> Result<Connection, Box<dyn Error>> {
    let own = fake.own.clone();
    let mut builder = Builder::session()?;
    if owns_name {
        builder = builder.name(DBUS_SERVICE_NAME)?;
    }
    let connection = builder.serve_at(DBUS_OBJECT_PATH, fake)?.build().await?;
    let _ = own.set(connection.clone());
    Ok(connection)
}

async fn emit(connection: &Connection, event: &Event) -> Result<(), Box<dyn Error>> {
    let interface = connection
        .object_server()
        .interface::<_, Fake>(DBUS_OBJECT_PATH)
        .await?;
    Fake::event(interface.signal_emitter(), &encode_event(event)?).await?;
    Ok(())
}

/// Sets its flag when dropped, so a held fake reply is released however a test ends.
struct ReleaseOnDrop(Arc<AtomicBool>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

// Calls to A's Version: 1 at connect, 2 when subscribing, 3 for the event's own check.

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_02_an_event_whose_owner_hands_over_during_its_version_check_is_not_delivered()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let a = Fake::new(Some(3), false);
        let successor = a.successor.clone();
        let b_conn = serve(Fake::new(None, false), false).await?;
        let _ = successor.set(b_conn.clone());
        let a_conn = serve(a, true).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.changes().await?;

        // A emits, then retires itself inside the Version check of that very event.
        emit(&a_conn, &Event::HealthChanged(HealthLevel::Degraded)).await?;
        let first = future::or(async { Some(events.next_change().await) }, async {
            Timer::after(Duration::from_secs(2)).await;
            None
        })
        .await;
        match first {
            Some(Err(blueroute_client::ClientError::OwnerLost)) => {}
            other => panic!("A's obsolete event was not retired: {other:?}"),
        }
        drop((a_conn, b_conn));
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_02_owner_loss_is_reported_while_the_events_version_reply_is_held()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let a = Fake::new(Some(3), true);
        let _release = ReleaseOnDrop(a.release.clone());
        let release = a.release.clone();
        let successor = a.successor.clone();
        let b_conn = serve(Fake::new(None, false), false).await?;
        let _ = successor.set(b_conn.clone());
        let a_conn = serve(a, true).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.changes().await?;

        emit(&a_conn, &Event::HealthChanged(HealthLevel::Degraded)).await?;
        // A stays connected and does not answer: the loss must not wait for it.
        let first = future::or(async { Some(events.next_change().await) }, async {
            Timer::after(Duration::from_secs(2)).await;
            None
        })
        .await;
        match first {
            Some(Err(blueroute_client::ClientError::OwnerLost)) => {}
            other => panic!("owner loss not reported within 2 s of the handover: {other:?}"),
        }
        // A's late reply and further events change nothing: retirement is terminal.
        release.store(true, Ordering::SeqCst);
        emit(&a_conn, &Event::HealthChanged(HealthLevel::Error)).await?;
        let later = future::or(async { Some(events.next_change().await) }, async {
            Timer::after(Duration::from_millis(500)).await;
            None
        })
        .await;
        assert!(
            !matches!(later, Some(Ok(_))),
            "a retired subscription delivered: {later:?}"
        );
        drop((a_conn, b_conn));
        Ok(())
    })
}

async fn within<T>(bound: Duration, work: impl std::future::Future<Output = T>) -> Option<T> {
    future::or(async { Some(work.await) }, async {
        Timer::after(bound).await;
        None
    })
    .await
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_02_an_incompatible_successor_after_an_event_time_handover_is_never_used()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let a = Fake::new(Some(3), false);
        let successor = a.successor.clone();
        let mut incompatible = Fake::new(None, false);
        incompatible.version = (99, 0);
        let b_requests = incompatible.requests.clone();
        let b_conn = serve(incompatible, false).await?;
        let _ = successor.set(b_conn.clone());
        let a_conn = serve(a, true).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.changes().await?;
        emit(&a_conn, &Event::HealthChanged(HealthLevel::Degraded)).await?;
        assert!(matches!(
            within(Duration::from_secs(2), events.next_change()).await,
            Some(Err(blueroute_client::ClientError::OwnerLost))
        ));
        // Recovery tries B, which is refused before any ordinary request reaches it.
        assert!(client.changes().await.is_err());
        let refused = client
            .request(&blueroute_protocol::Command::GetStatus)
            .await;
        assert!(refused.is_err(), "{refused:?}");
        assert_eq!(b_requests.load(Ordering::SeqCst), 0);
        drop((a_conn, b_conn));
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_02_an_owner_leaving_with_no_successor_during_validation_is_reported()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let a = Fake::new(Some(3), true); // no successor; the reply is held
        let _release = ReleaseOnDrop(a.release.clone());
        let a_conn = serve(a, true).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.changes().await?;
        emit(&a_conn, &Event::HealthChanged(HealthLevel::Degraded)).await?;
        assert!(matches!(
            within(Duration::from_secs(2), events.next_change()).await,
            Some(Err(blueroute_client::ClientError::OwnerLost))
        ));
        drop(a_conn);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_02_an_interrupted_wait_loses_nothing_and_a_fresh_subscription_follows_b()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let a_conn = serve(Fake::new(None, false), true).await?;
        let b_conn = serve(Fake::new(None, false), false).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.changes().await?;
        // A caller gives up waiting (the future is dropped) …
        assert!(
            within(Duration::from_millis(100), events.next_change())
                .await
                .is_none()
        );
        // … then A hands over to B: the next wait still sees the loss.
        a_conn.release_name(DBUS_SERVICE_NAME).await?;
        b_conn.request_name(DBUS_SERVICE_NAME).await?;
        assert!(matches!(
            within(Duration::from_secs(2), events.next_change()).await,
            Some(Err(blueroute_client::ClientError::OwnerLost))
        ));
        // Retirement is terminal, even if A emits again.
        emit(&a_conn, &Event::HealthChanged(HealthLevel::Error)).await?;
        assert!(matches!(
            events.next_change().await,
            Err(blueroute_client::ClientError::OwnerLost)
        ));
        // A fresh subscription binds to B and hears B.
        let mut fresh = client.changes().await?;
        emit(&b_conn, &Event::HealthChanged(HealthLevel::Degraded)).await?;
        assert!(matches!(
            within(Duration::from_secs(2), fresh.next_change()).await,
            Some(Ok(blueroute_client::Change::Event(Event::HealthChanged(
                HealthLevel::Degraded
            ))))
        ));
        drop((a_conn, b_conn));
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn fu_02_the_shared_watcher_moves_to_b_after_an_event_time_handover() -> Result<(), Box<dyn Error>>
{
    use blueroute_client::WatchSignal::{Changed, Connected, Disconnected};
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        // Watcher connect = Version 1, subscribe = 2, A's event check = 3.
        let a = Fake::new(Some(3), false);
        let successor = a.successor.clone();
        let b_conn = serve(Fake::new(None, false), false).await?;
        let _ = successor.set(b_conn.clone());
        let a_conn = serve(a, true).await?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let watcher = blueroute_client::watch(
            || async {
                let connection = Connection::session()
                    .await
                    .map_err(blueroute_client::ClientError::Bus)?;
                BlueRouteClient::from_connection(connection).await
            },
            |_| async {
                Timer::after(Duration::from_millis(10)).await;
            },
            move |signal| {
                sink.lock().unwrap().push(signal);
                std::ops::ControlFlow::Continue(())
            },
        );
        let scenario = async {
            let wait_for = |expected: Vec<blueroute_client::WatchSignal>| {
                let seen = seen.clone();
                async move {
                    for _ in 0..200 {
                        if *seen.lock().unwrap() == expected {
                            return true;
                        }
                        Timer::after(Duration::from_millis(10)).await;
                    }
                    false
                }
            };
            assert!(wait_for(vec![Connected]).await);
            emit(&a_conn, &Event::HealthChanged(HealthLevel::Degraded)).await?;
            // A's obsolete event is not reported as a change; the watcher reconnects to B.
            assert!(
                wait_for(vec![Connected, Disconnected, Connected]).await,
                "{:?}",
                seen.lock().unwrap()
            );
            emit(&b_conn, &Event::HealthChanged(HealthLevel::Error)).await?;
            assert!(
                wait_for(vec![Connected, Disconnected, Connected, Changed]).await,
                "{:?}",
                seen.lock().unwrap()
            );
            Ok::<(), Box<dyn Error>>(())
        };
        future::or(
            async {
                watcher.await;
                Err("the watcher stopped".into())
            },
            scenario,
        )
        .await?;
        drop((a_conn, b_conn));
        Ok(())
    })
}

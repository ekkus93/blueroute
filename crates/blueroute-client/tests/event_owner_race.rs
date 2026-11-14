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
            let successor = self.successor.get().expect("successor connection is set");
            own.release_name(DBUS_SERVICE_NAME).await.expect("release");
            successor
                .request_name(DBUS_SERVICE_NAME)
                .await
                .expect("successor takes the name");
            while self.hold && !self.release.load(Ordering::SeqCst) {
                Timer::after(Duration::from_millis(5)).await;
            }
        }
        self.version
    }

    async fn request(&self, _payload: &str) -> fdo::Result<String> {
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
#[ignore = "FU-02 regression: fails on 6d200d0 until FU-02 is fixed; needs dbus-run-session"]
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
#[ignore = "FU-02 regression: fails on 6d200d0 until FU-02 is fixed; needs dbus-run-session"]
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

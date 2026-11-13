//! CR-F8 regressions (docs/CODE_REVIEW_REMEDIATION_TODO.md, OWN-1..OWN-4).
//!
//! A command must reach the same daemon owner whose compatible version was just checked, and an
//! event subscription must stay with that owner. On baseline 4ca5042 `request()` checked
//! `Version` and then sent `Request` to the well-known name again, so a replacement owner could
//! receive it. The handovers happen inside the fakes' own handlers, so each race is
//! deterministic, not timing luck. They need a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test owner_race --locked -- --ignored`
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_io::Timer;
use blueroute_client::{BlueRouteClient, ClientErrorKind};
use blueroute_core::{DisplayName, HealthLevel};
use blueroute_protocol::{
    ApiVersion, Command, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event, encode_event,
};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, fdo, interface};

/// Every test competes for the same well-known name on the one private bus, so they must not
/// run concurrently.
static BUS: Mutex<()> = Mutex::new(());

const COMPATIBLE: (u16, u16) = (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor);
const INCOMPATIBLE: (u16, u16) = (99, 0);

/// What a fake does at a chosen moment, to open one race window deterministically.
#[derive(Clone, Copy)]
enum Act {
    Nothing,
    /// On this Version call: give the well-known name to the successor, then reply.
    HandOverOnVersion(usize),
    /// On this Version call: close this daemon's connection before replying.
    VanishOnVersion(usize),
    /// On every Request: never reply (the test then closes this daemon's connection).
    NeverReplyOnRequest,
}

struct Fake {
    version: (u16, u16),
    act: Act,
    version_calls: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    own: Arc<OnceLock<Connection>>,
    successor: Arc<OnceLock<Connection>>,
}

impl Fake {
    fn new(version: (u16, u16), act: Act) -> Self {
        Self {
            version,
            act,
            version_calls: Arc::default(),
            requests: Arc::default(),
            own: Arc::default(),
            successor: Arc::default(),
        }
    }
}

#[interface(name = "org.blueroute.Service1")]
impl Fake {
    async fn version(&self) -> (u16, u16) {
        let call = self.version_calls.fetch_add(1, Ordering::SeqCst) + 1;
        match self.act {
            Act::HandOverOnVersion(on) if on == call => {
                let own = self.own.get().expect("own connection is set");
                let successor = self.successor.get().expect("successor connection is set");
                own.release_name(DBUS_SERVICE_NAME).await.expect("release");
                successor
                    .request_name(DBUS_SERVICE_NAME)
                    .await
                    .expect("successor takes the name");
            }
            Act::VanishOnVersion(on) if on == call => {
                let own = self.own.get().expect("own connection is set").clone();
                let _ = own.close().await;
            }
            _ => {}
        }
        self.version
    }

    async fn request(&self, _payload: &str) -> fdo::Result<String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if let Act::NeverReplyOnRequest = self.act {
            std::future::pending::<()>().await;
        }
        Ok(r#"{"type":"ack"}"#.to_owned())
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;
}

/// Serve `fake` on its own connection; `owns_name` decides whether it holds the name now.
async fn serve(
    fake: Fake,
    owns_name: bool,
) -> Result<(Connection, Arc<AtomicUsize>, Arc<AtomicUsize>), Box<dyn Error>> {
    let requests = fake.requests.clone();
    let version_calls = fake.version_calls.clone();
    let own = fake.own.clone();
    let mut builder = Builder::session()?;
    if owns_name {
        builder = builder.name(DBUS_SERVICE_NAME)?;
    }
    let connection = builder.serve_at(DBUS_OBJECT_PATH, fake)?.build().await?;
    let _ = own.set(connection.clone());
    Ok((connection, requests, version_calls))
}

fn create() -> Command {
    Command::CreateNetwork {
        name: DisplayName::new("Owner race").unwrap(),
    }
}

async fn emit(connection: &Connection, event: &Event) -> Result<(), Box<dyn Error>> {
    let interface = connection
        .object_server()
        .interface::<_, Fake>(DBUS_OBJECT_PATH)
        .await?;
    Fake::event(interface.signal_emitter(), &encode_event(event)?).await?;
    Ok(())
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f8_incompatible_replacement_during_version_check_receives_no_command()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        // Call 1 is the connect-time check; call 2 is the check right before the command.
        let a = Fake::new(COMPATIBLE, Act::HandOverOnVersion(2));
        let successor = a.successor.clone();
        let (b_conn, b_requests, _) = serve(Fake::new(INCOMPATIBLE, Act::Nothing), false).await?;
        let _ = successor.set(b_conn.clone());
        let (a_conn, a_requests, _) = serve(a, true).await?;

        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let outcome = client.request(&create()).await;

        assert_eq!(
            b_requests.load(Ordering::SeqCst),
            0,
            "the incompatible replacement received a normal command (outcome: {outcome:?})"
        );
        // OWN-2: A relinquished the name but is still connected, so finishing on A, the owner
        // that was validated, is allowed. It must not migrate to B.
        assert_eq!(a_requests.load(Ordering::SeqCst), 1);
        assert!(outcome.is_ok(), "{outcome:?}");
        drop((a_conn, b_conn));
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f8_after_a_compatible_replacement_the_next_command_goes_to_the_new_owner()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let (a_conn, a_requests, _) = serve(Fake::new(COMPATIBLE, Act::Nothing), true).await?;
        let (b_conn, b_requests, _) = serve(Fake::new(COMPATIBLE, Act::Nothing), false).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        client.request(&create()).await?;
        assert_eq!(a_requests.load(Ordering::SeqCst), 1);

        // A is fully replaced by a compatible B between operations.
        a_conn.release_name(DBUS_SERVICE_NAME).await?;
        drop(a_conn);
        b_conn.request_name(DBUS_SERVICE_NAME).await?;
        client.request(&create()).await?;
        assert_eq!(b_requests.load(Ordering::SeqCst), 1);
        assert_eq!(a_requests.load(Ordering::SeqCst), 1);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f8_an_owner_vanishing_before_dispatch_fails_closed_and_reaches_no_one()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let a = Fake::new(COMPATIBLE, Act::VanishOnVersion(2));
        let (a_conn, a_requests, _) = serve(a, true).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let outcome = client.request(&create()).await;
        let error = outcome.expect_err("a vanished owner cannot accept the command");
        assert_eq!(error.kind(), ClientErrorKind::Unavailable, "{error}");
        assert_eq!(a_requests.load(Ordering::SeqCst), 0);
        drop(a_conn);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f8_a_lost_reply_is_reported_and_never_replayed() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let (a_conn, a_requests, _) =
            serve(Fake::new(COMPATIBLE, Act::NeverReplyOnRequest), true).await?;
        // A compatible successor is ready, so a blind retry would have somewhere to go.
        let (b_conn, b_requests, _) = serve(Fake::new(COMPATIBLE, Act::Nothing), false).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        // Once A has received the request, A goes away without ever replying, and a compatible
        // B takes the name.
        let closer = a_conn.clone();
        let successor = b_conn.clone();
        let delivered = a_requests.clone();
        let (outcome, ()) = future::zip(client.request(&create()), async move {
            while delivered.load(Ordering::SeqCst) == 0 {
                Timer::after(Duration::from_millis(5)).await;
            }
            let _ = closer.close().await;
            let _ = successor.request_name(DBUS_SERVICE_NAME).await;
        })
        .await;
        assert!(
            outcome.is_err(),
            "a lost reply is not a success: {outcome:?}"
        );
        assert_eq!(
            a_requests.load(Ordering::SeqCst),
            1,
            "delivered exactly once"
        );
        assert_eq!(
            b_requests.load(Ordering::SeqCst),
            0,
            "never replayed elsewhere"
        );
        drop((a_conn, b_conn));
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f8_a_subscription_never_delivers_a_replacement_owners_events() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let (a_conn, _, _) = serve(Fake::new(COMPATIBLE, Act::Nothing), true).await?;
        let (b_conn, _, _) = serve(Fake::new(COMPATIBLE, Act::Nothing), false).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.events().await?;

        // A hands the name to B but stays connected; B emits an event.
        a_conn.release_name(DBUS_SERVICE_NAME).await?;
        b_conn.request_name(DBUS_SERVICE_NAME).await?;
        emit(&b_conn, &Event::HealthChanged(HealthLevel::Error)).await?;
        // A, the owner the subscription was validated against, emits too.
        emit(&a_conn, &Event::HealthChanged(HealthLevel::Degraded)).await?;

        let first = future::or(async { Some(events.next_event().await) }, async {
            Timer::after(Duration::from_secs(2)).await;
            None
        })
        .await;
        // The name moved away from A, so the subscription ends (CR-F2, WATCH-1); B's event is
        // never delivered on it, and nothing arrives under A's earlier version check.
        match first {
            Some(Err(error)) => assert!(
                matches!(error, blueroute_client::ClientError::OwnerLost),
                "{error}"
            ),
            other => panic!("expected the subscription to end with OwnerLost, got {other:?}"),
        }
        drop((a_conn, b_conn));
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_an_owner_change_during_subscription_setup_is_not_missed() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        // Call 1 is the connect-time check; call 2 is the check inside events(), after the
        // owner was resolved and before the subscription is returned. A hands over right there.
        let a = Fake::new(COMPATIBLE, Act::HandOverOnVersion(2));
        let successor = a.successor.clone();
        let (b_conn, _, _) = serve(Fake::new(COMPATIBLE, Act::Nothing), false).await?;
        let _ = successor.set(b_conn.clone());
        let (a_conn, _, _) = serve(a, true).await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.events().await?;

        // No event is ever emitted: the change observed during setup alone ends it.
        let first = future::or(async { Some(events.next_event().await) }, async {
            Timer::after(Duration::from_secs(2)).await;
            None
        })
        .await;
        match first {
            Some(Err(blueroute_client::ClientError::OwnerLost)) => {}
            other => panic!("the handover during setup was missed: {other:?}"),
        }
        drop((a_conn, b_conn));
        Ok(())
    })
}

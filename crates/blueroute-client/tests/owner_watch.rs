//! CR-F2 regressions (docs/CODE_REVIEW_REMEDIATION_TODO.md, WATCH-1/WATCH-4).
//!
//! An event subscription must notice an idle daemon's disappearance without any application
//! event. These tests state the corrected contract and fail on baseline 4ca5042, where
//! `next_event()` waits forever because nothing observes the service owner. They need a private
//! bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test owner_watch --locked -- --ignored`
use std::error::Error;
use std::sync::Mutex;
use std::time::Duration;

use async_io::Timer;
use blueroute_client::BlueRouteClient;
use blueroute_core::HealthLevel;
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event, encode_event};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, fdo, interface};

/// The tests share the daemon's well-known name on one private bus, so they run one at a time.
static BUS: Mutex<()> = Mutex::new(());

/// WATCH-4: in a controlled local test, owner loss must surface within this bound.
const LOSS_BOUND: Duration = Duration::from_secs(2);

struct IdleDaemon;

#[interface(name = "org.blueroute.Service1")]
impl IdleDaemon {
    fn version(&self) -> (u16, u16) {
        (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor)
    }

    fn request(&self, _payload: &str) -> fdo::Result<String> {
        Ok(r#"{"type":"ack"}"#.to_owned())
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;
}

async fn emit(connection: &Connection, event: &Event) -> Result<(), Box<dyn Error>> {
    let interface = connection
        .object_server()
        .interface::<_, IdleDaemon>(DBUS_OBJECT_PATH)
        .await?;
    IdleDaemon::event(interface.signal_emitter(), &encode_event(event)?).await?;
    Ok(())
}

/// Wait at most `bound` for the next subscription result.
async fn within<T>(bound: Duration, work: impl std::future::Future<Output = T>) -> Option<T> {
    future::or(async { Some(work.await) }, async {
        Timer::after(bound).await;
        None
    })
    .await
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_idle_daemon_loss_ends_the_event_subscription_without_any_event()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let daemon = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, IdleDaemon)?
            .build()
            .await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.events().await?;

        // The daemon goes away without ever emitting an Event.
        daemon.release_name(DBUS_SERVICE_NAME).await?;
        drop(daemon);

        let outcome = future::or(async { Some(events.next_event().await) }, async {
            Timer::after(LOSS_BOUND).await;
            None
        })
        .await;
        match outcome {
            Some(Err(error)) => {
                assert!(
                    matches!(error, blueroute_client::ClientError::OwnerLost),
                    "{error}"
                );
                Ok(())
            }
            Some(Ok(event)) => panic!("a vanished daemon produced an event: {event:?}"),
            None => panic!("the subscription did not notice the owner loss within {LOSS_BOUND:?}"),
        }
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_an_unrelated_name_changing_hands_does_not_disturb_the_subscription()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let daemon = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, IdleDaemon)?
            .build()
            .await?;
        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let mut events = client.events().await?;

        // Some other service on the bus appears and goes away.
        let other = Connection::session().await?;
        other.request_name("org.example.Unrelated").await?;
        other.release_name("org.example.Unrelated").await?;
        drop(other);

        emit(&daemon, &Event::HealthChanged(HealthLevel::Degraded)).await?;
        match within(LOSS_BOUND, events.next_event()).await {
            Some(Ok(Event::HealthChanged(HealthLevel::Degraded))) => Ok(()),
            other => panic!("the subscription was disturbed: {other:?}"),
        }
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f2_losing_the_bus_connection_ends_the_subscription() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let _daemon = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, IdleDaemon)?
            .build()
            .await?;
        // A private connection, so closing it cannot affect the other tests' connections.
        let connection = Builder::session()?.build().await?;
        let client = BlueRouteClient::from_connection(connection.clone()).await?;
        let mut events = client.events().await?;
        connection.close().await?;
        match within(LOSS_BOUND, events.next_event()).await {
            Some(Err(_)) => Ok(()),
            other => panic!("a closed connection must end the subscription: {other:?}"),
        }
    })
}

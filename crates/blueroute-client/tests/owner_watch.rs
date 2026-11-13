//! CR-F2 regressions (docs/CODE_REVIEW_REMEDIATION_TODO.md, WATCH-1/WATCH-4).
//!
//! An event subscription must notice an idle daemon's disappearance without any application
//! event. These tests state the corrected contract and fail on baseline 4ca5042, where
//! `next_event()` waits forever because nothing observes the service owner. They need a private
//! bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test owner_watch --locked -- --ignored`
use std::error::Error;
use std::time::Duration;

use async_io::Timer;
use blueroute_client::BlueRouteClient;
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::{Connection, fdo, interface};

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
}

#[test]
#[ignore = "CR-F2 regression: fails on baseline 4ca5042 until CR-F2 is fixed; needs dbus-run-session"]
fn cr_f2_idle_daemon_loss_ends_the_event_subscription_without_any_event()
-> Result<(), Box<dyn Error>> {
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
            Some(Err(_)) => Ok(()),
            Some(Ok(event)) => panic!("a vanished daemon produced an event: {event:?}"),
            None => panic!("the subscription did not notice the owner loss within {LOSS_BOUND:?}"),
        }
    })
}

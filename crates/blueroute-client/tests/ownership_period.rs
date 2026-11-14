//! LC-BASE / LC-OWNER regressions (docs/CODE_REVIEW_LIFECYCLE_TODO.md, RFU-01, EPOCH-1..5): a
//! subscription belongs to one continuous ownership period of the daemon's name. On 75c1a01 an
//! owner that gives the name away and takes it back (A→B→A, A→unowned→A) still delivers its old
//! item, because the final check only compares the current owner's unique name.
//!
//! The daemon here is a raw message loop, so a test knows exactly when a Version reply has been
//! sent. Ordering is established with acknowledgements, not sleeps: after replying, the daemon
//! calls a barrier object on the client's own connection. That call travels behind the reply on
//! the same socket, so once it is answered the reply and every earlier ownership notice are
//! queued at the client. Needs a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test ownership_period --locked -- --ignored`
use std::error::Error;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_io::Timer;
use blueroute_client::{BlueRouteClient, Change, ClientError};
use blueroute_core::HealthLevel;
use blueroute_protocol::{
    ApiVersion, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event, encode_event,
};
use futures_lite::{StreamExt, future};
use zbus::message::Type;
use zbus::names::OwnedUniqueName;
use zbus::{Connection, MessageStream, interface};

/// The tests share the daemon's well-known name on one private bus.
static BUS: Mutex<()> = Mutex::new(());

const COMPATIBLE: (u16, u16) = (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor);
const BARRIER_PATH: &str = "/org/blueroute/test/Barrier";

/// What the daemon does inside one Version call before replying.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Transfer {
    /// A→B→A: B takes the name, gives it back, and A takes it again.
    ThroughSuccessor,
    /// A→unowned→A: A releases the name and takes it again.
    ThroughNoOwner,
}

/// Shared progress of the scripted daemon.
#[derive(Default)]
struct Script {
    version_calls: AtomicUsize,
    requests: AtomicUsize,
    /// The Version call (1-based) inside which `transfer` happens.
    at: AtomicUsize,
    /// Set when that call has arrived; the daemon then waits for `go`.
    arrived: AtomicBool,
    go: AtomicBool,
    /// Set once the reply is sent and the client has acknowledged the barrier behind it.
    replied: AtomicBool,
}

struct Daemon {
    connection: Connection,
    successor: Connection,
    transfer: Transfer,
    script: Arc<Script>,
    /// The client connection's unique name, for the barrier call.
    client: OwnedUniqueName,
    /// The version this daemon reports.
    version: (u16, u16),
}

impl Daemon {
    async fn serve(self) -> Result<(), Box<dyn Error>> {
        let mut calls = MessageStream::from(&self.connection);
        while let Some(message) = calls.next().await {
            let message = message?;
            let header = message.header();
            if message.message_type() != Type::MethodCall
                || header.interface().map(|name| name.as_str()) != Some(DBUS_INTERFACE_NAME)
            {
                continue;
            }
            match header.member().map(|name| name.as_str()) {
                Some("Version") => {
                    let call = self.script.version_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let scripted = self.script.at.load(Ordering::SeqCst) == call;
                    if scripted {
                        self.script.arrived.store(true, Ordering::SeqCst);
                        wait_for(&self.script.go).await;
                        self.move_name().await?;
                    }
                    let reply = zbus::Message::method_return(&header)?.build(&self.version)?;
                    self.connection.send(&reply).await?;
                    if scripted {
                        // Behind the reply on this socket: answered only once the reply is queued.
                        self.connection
                            .call_method(
                                Some(self.client.as_str()),
                                BARRIER_PATH,
                                Some("org.blueroute.Test"),
                                "Ping",
                                &(),
                            )
                            .await?;
                        self.script.replied.store(true, Ordering::SeqCst);
                    }
                }
                Some("Request") => {
                    self.script.requests.fetch_add(1, Ordering::SeqCst);
                    let reply =
                        zbus::Message::method_return(&header)?.build(&r#"{"type":"ack"}"#)?;
                    self.connection.send(&reply).await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn move_name(&self) -> Result<(), Box<dyn Error>> {
        self.connection.release_name(DBUS_SERVICE_NAME).await?;
        if self.transfer == Transfer::ThroughSuccessor {
            self.successor.request_name(DBUS_SERVICE_NAME).await?;
            self.successor.release_name(DBUS_SERVICE_NAME).await?;
        }
        self.connection.request_name(DBUS_SERVICE_NAME).await?;
        Ok(())
    }
}

struct Barrier;

#[interface(name = "org.blueroute.Test")]
impl Barrier {
    fn ping(&self) {}
}

async fn wait_for(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        Timer::after(Duration::from_millis(2)).await;
    }
}

async fn within<T>(bound: Duration, work: impl Future<Output = T>) -> Option<T> {
    future::or(async { Some(work.await) }, async {
        Timer::after(bound).await;
        None
    })
    .await
}

/// Which kind of item the old owner emits.
#[derive(Clone, Copy, Debug)]
enum Item {
    Event,
    State,
}

async fn emit(connection: &Connection, item: Item) -> Result<(), Box<dyn Error>> {
    match item {
        Item::Event => {
            let payload = encode_event(&Event::HealthChanged(HealthLevel::Degraded))?;
            connection
                .emit_signal(
                    None::<&str>,
                    DBUS_OBJECT_PATH,
                    DBUS_INTERFACE_NAME,
                    "Event",
                    &payload,
                )
                .await?;
        }
        Item::State => {
            connection
                .emit_signal(
                    None::<&str>,
                    DBUS_OBJECT_PATH,
                    DBUS_INTERFACE_NAME,
                    "StateChanged",
                    &7_u64,
                )
                .await?;
        }
    }
    Ok(())
}

/// One scenario's bus: daemon A owning the name, a spare connection B, and a client whose own
/// connection serves the barrier.
struct Rig {
    a: Connection,
    b: Connection,
    client_connection: Connection,
    script: Arc<Script>,
}

async fn rig() -> Result<Rig, Box<dyn Error>> {
    let a = Connection::session().await?;
    a.request_name(DBUS_SERVICE_NAME).await?;
    let b = Connection::session().await?;
    let client_connection = Connection::session().await?;
    client_connection
        .object_server()
        .at(BARRIER_PATH, Barrier)
        .await?;
    Ok(Rig {
        a,
        b,
        client_connection,
        script: Arc::default(),
    })
}

impl Rig {
    fn daemon(&self, transfer: Transfer) -> Daemon {
        Daemon {
            connection: self.a.clone(),
            successor: self.b.clone(),
            transfer,
            script: self.script.clone(),
            client: self
                .client_connection
                .unique_name()
                .expect("a bus connection has a unique name")
                .to_owned(),
            version: COMPATIBLE,
        }
    }
}

/// Runs `scenario` while the scripted daemon serves; the daemon stops with the scenario.
async fn with_daemon(
    daemon: Daemon,
    scenario: impl Future<Output = Result<(), Box<dyn Error>>>,
) -> Result<(), Box<dyn Error>> {
    future::or(
        async {
            daemon.serve().await?;
            Err("the daemon stopped".into())
        },
        scenario,
    )
    .await
}

/// RFU-01 / LC-BASE-03 / LC-OWNER-04: A emits, and inside that item's Version check gives the
/// name to B and takes it back before replying. The consumer is paused until the reply and all
/// ownership notices are queued. The old subscription must retire first, stay retired, and a
/// fresh subscription must hear A's new period.
fn returning_owner_case(item: Item) -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        let daemon = rig.daemon(Transfer::ThroughSuccessor);
        with_daemon(daemon, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            // Version calls: 1 at connect, 2 when subscribing, 3 for the item's own check.
            rig.script.at.store(3, Ordering::SeqCst);
            emit(&rig.a, item).await?;

            {
                let mut next = pin!(old.next_change());
                // Poll only until the item's Version call has reached A …
                while !rig.script.arrived.load(Ordering::SeqCst) {
                    if let Some(early) = future::poll_once(next.as_mut()).await {
                        return Err(format!("delivered before its check: {early:?}").into());
                    }
                    Timer::after(Duration::from_millis(2)).await;
                }
                // … then stay paused while A goes A→B→A, replies, and the reply is queued.
                rig.script.go.store(true, Ordering::SeqCst);
                wait_for(&rig.script.replied).await;
                match within(Duration::from_secs(2), next).await {
                    Some(Err(ClientError::OwnerLost)) => {}
                    other => {
                        return Err(format!(
                            "the old period's {item:?} was not retired: {other:?}"
                        )
                        .into());
                    }
                }
            }
            // Every later wait on the old subscription stays retired, even with A emitting again.
            emit(&rig.a, item).await?;
            assert!(matches!(
                within(Duration::from_secs(1), old.next_change()).await,
                Some(Err(ClientError::OwnerLost))
            ));
            // A fresh subscription is validated against the returning A and hears its new state.
            let mut fresh = client.changes().await?;
            emit(&rig.a, item).await?;
            match within(Duration::from_secs(2), fresh.next_change()).await {
                Some(Ok(Change::Event(Event::HealthChanged(HealthLevel::Degraded)))) => {}
                Some(Ok(Change::State { revision: 7 })) => {}
                other => return Err(format!("the fresh subscription missed A: {other:?}").into()),
            }
            Ok(())
        })
        .await
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_returning_owner_does_not_deliver_its_obsolete_event() -> Result<(), Box<dyn Error>> {
    returning_owner_case(Item::Event)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_returning_owner_does_not_deliver_its_obsolete_state_change()
-> Result<(), Box<dyn Error>> {
    returning_owner_case(Item::State)
}

/// LC-OWNER-05: A→unowned→A with A's old signal already queued before the subscription selects
/// it. GetNameOwner names A again, yet the old period ended.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_signal_queued_before_a_release_and_reacquire_is_not_delivered()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        let daemon = rig.daemon(Transfer::ThroughNoOwner);
        with_daemon(daemon, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            for item in [Item::Event, Item::State] {
                emit(&rig.a, item).await?;
            }
            rig.a.release_name(DBUS_SERVICE_NAME).await?;
            rig.a.request_name(DBUS_SERVICE_NAME).await?;
            // A broker round trip on the client's connection: the signals and both notices are
            // now queued there.
            zbus::fdo::DBusProxy::new(&rig.client_connection)
                .await?
                .get_id()
                .await?;
            match within(Duration::from_secs(2), old.next_change()).await {
                Some(Err(ClientError::OwnerLost)) => {}
                other => return Err(format!("the old period delivered: {other:?}").into()),
            }
            assert!(matches!(
                old.next_change().await,
                Err(ClientError::OwnerLost)
            ));
            Ok(())
        })
        .await
    })
}

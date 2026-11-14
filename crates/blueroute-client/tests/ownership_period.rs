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
    /// Move the name only after replying (inside the client's final verification), not before.
    after_reply: AtomicBool,
    /// After moving the name, hold the reply until `release`.
    hold: AtomicBool,
    release: AtomicBool,
    /// Report an incompatible version from now on.
    incompatible: AtomicBool,
}

struct Daemon {
    connection: Connection,
    successor: Connection,
    transfer: Transfer,
    script: Arc<Script>,
    /// The client connection's unique name, for the barrier call.
    client: OwnedUniqueName,
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
                    let after_reply = self.script.after_reply.load(Ordering::SeqCst);
                    if scripted {
                        self.script.arrived.store(true, Ordering::SeqCst);
                        wait_for(&self.script.go).await;
                        if !after_reply {
                            self.move_name().await?;
                        }
                        if self.script.hold.load(Ordering::SeqCst) {
                            wait_for(&self.script.release).await;
                        }
                    }
                    let version = if self.script.incompatible.load(Ordering::SeqCst) {
                        (99, 0)
                    } else {
                        COMPATIBLE
                    };
                    let reply = zbus::Message::method_return(&header)?.build(&version)?;
                    self.connection.send(&reply).await?;
                    if scripted {
                        if after_reply {
                            self.move_name().await?;
                        }
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

/// Polls `next` only until the scripted Version call reaches the daemon, then releases the
/// daemon and stays paused until its reply (and everything before it) is queued at the client.
async fn pause_across_the_script<T: std::fmt::Debug>(
    script: &Script,
    mut next: std::pin::Pin<&mut impl Future<Output = T>>,
) -> Result<(), Box<dyn Error>> {
    while !script.arrived.load(Ordering::SeqCst) {
        if let Some(early) = future::poll_once(next.as_mut()).await {
            return Err(format!("finished before its check: {early:?}").into());
        }
        Timer::after(Duration::from_millis(2)).await;
    }
    script.go.store(true, Ordering::SeqCst);
    wait_for(&script.replied).await;
    Ok(())
}

/// LC-OWNER-05: the period ends after A has replied to the item's Version check, while the
/// client's final `GetNameOwner` verification is pending. The name is A's again by then.
fn ends_during_final_verification(transfer: Transfer) -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        let daemon = rig.daemon(transfer);
        with_daemon(daemon, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            rig.script.after_reply.store(true, Ordering::SeqCst);
            rig.script.at.store(3, Ordering::SeqCst);
            emit(&rig.a, Item::Event).await?;
            let mut next = pin!(old.next_change());
            pause_across_the_script(&rig.script, next.as_mut()).await?;
            match within(Duration::from_secs(2), next).await {
                Some(Err(ClientError::OwnerLost)) => Ok(()),
                other => Err(format!("delivered across a {transfer:?}: {other:?}").into()),
            }
        })
        .await
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_handover_and_return_during_final_verification_is_not_delivered()
-> Result<(), Box<dyn Error>> {
    ends_during_final_verification(Transfer::ThroughSuccessor)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_release_and_reacquire_during_final_verification_is_not_delivered()
-> Result<(), Box<dyn Error>> {
    ends_during_final_verification(Transfer::ThroughNoOwner)
}

/// LC-OWNER-05/06: A goes A→B→A and then holds the item's Version reply. An actively polled
/// consumer learns of the loss within the two-second bound, without the reply; the late reply
/// changes nothing.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_held_reply_does_not_delay_the_end_of_the_period() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        let daemon = rig.daemon(Transfer::ThroughSuccessor);
        with_daemon(daemon, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            rig.script.hold.store(true, Ordering::SeqCst);
            rig.script.go.store(true, Ordering::SeqCst);
            rig.script.at.store(3, Ordering::SeqCst);
            emit(&rig.a, Item::State).await?;
            match within(Duration::from_secs(2), old.next_change()).await {
                Some(Err(ClientError::OwnerLost)) => {}
                other => return Err(format!("loss waited for the reply: {other:?}").into()),
            }
            assert!(
                !rig.script.replied.load(Ordering::SeqCst),
                "the reply was held"
            );
            rig.script.release.store(true, Ordering::SeqCst);
            wait_for(&rig.script.replied).await;
            emit(&rig.a, Item::State).await?;
            assert!(matches!(
                within(Duration::from_secs(1), old.next_change()).await,
                Some(Err(ClientError::OwnerLost))
            ));
            Ok(())
        })
        .await
    })
}

/// LC-OWNER-03/05: the caller abandons the wait while the item's Version check is pending; A
/// then goes A→B→A and replies. The next wait still reports the end of the period: cancelling
/// cannot erase it, and the abandoned item is never delivered.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_cancelling_during_validation_does_not_erase_the_end_of_the_period()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        let daemon = rig.daemon(Transfer::ThroughSuccessor);
        with_daemon(daemon, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            rig.script.at.store(3, Ordering::SeqCst);
            emit(&rig.a, Item::Event).await?;
            {
                let mut next = pin!(old.next_change());
                while !rig.script.arrived.load(Ordering::SeqCst) {
                    if let Some(early) = future::poll_once(next.as_mut()).await {
                        return Err(format!("finished before its check: {early:?}").into());
                    }
                    Timer::after(Duration::from_millis(2)).await;
                }
            } // the wait is dropped here, mid-validation
            rig.script.go.store(true, Ordering::SeqCst);
            wait_for(&rig.script.replied).await;
            match within(Duration::from_secs(2), old.next_change()).await {
                Some(Err(ClientError::OwnerLost)) => Ok(()),
                other => Err(format!("the cancelled period was revived: {other:?}").into()),
            }
        })
        .await
    })
}

/// LC-OWNER-06/07: a fresh subscription to the returning A is validated again. Once A reports
/// an incompatible version it is refused, and no ordinary request reaches it.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_returning_owner_is_validated_again_before_a_fresh_subscription()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        let daemon = rig.daemon(Transfer::ThroughSuccessor);
        with_daemon(daemon, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            rig.script.at.store(3, Ordering::SeqCst);
            emit(&rig.a, Item::Event).await?;
            let mut next = pin!(old.next_change());
            pause_across_the_script(&rig.script, next.as_mut()).await?;
            assert!(matches!(
                within(Duration::from_secs(2), next).await,
                Some(Err(ClientError::OwnerLost))
            ));
            rig.script.incompatible.store(true, Ordering::SeqCst);
            assert!(matches!(
                client.changes().await,
                Err(ClientError::IncompatibleVersion { .. })
            ));
            let refused = client
                .request(&blueroute_protocol::Command::GetStatus)
                .await;
            assert!(refused.is_err(), "{refused:?}");
            assert_eq!(rig.script.requests.load(Ordering::SeqCst), 0);
            Ok(())
        })
        .await
    })
}

/// LC-OWNER-06: a continuous period keeps delivering every item, in order, with no ownership
/// change at all; its own acquisition notice does not end it.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_owner_a_continuous_period_keeps_delivering() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        let daemon = rig.daemon(Transfer::ThroughNoOwner);
        with_daemon(daemon, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut changes = client.changes().await?;
            for item in [Item::Event, Item::State, Item::Event, Item::State] {
                emit(&rig.a, item).await?;
                let delivered = within(Duration::from_secs(2), changes.next_change()).await;
                assert!(
                    matches!(
                        (&delivered, item),
                        (Some(Ok(Change::Event(_))), Item::Event)
                            | (Some(Ok(Change::State { revision: 7 })), Item::State)
                    ),
                    "{item:?}: {delivered:?}"
                );
            }
            // Ordinary requests still go to A while its period lasts.
            client
                .request(&blueroute_protocol::Command::GetStatus)
                .await
                .ok();
            assert_eq!(rig.script.requests.load(Ordering::SeqCst), 1);
            Ok(())
        })
        .await
    })
}

/// Runs the scripted daemon on its own thread until dropped, so the test thread can block.
struct DaemonThread {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl DaemonThread {
    fn spawn(daemon: Daemon) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            future::block_on(future::or(
                async { daemon.serve().await.map_err(|error| error.to_string()) },
                async {
                    wait_for(&stopped).await;
                    Ok(())
                },
            ))
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for DaemonThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// LC-INTEGRATION-01: the production shared watcher sees A→B→A inside the check of A's item,
/// while its thread is busy: the test blocks the watcher's executor thread from the moment the
/// Version call reaches A until the reply and all notices are queued (acknowledged by the
/// barrier). It must report the loss before anything of the new period and never pass the old
/// item on. It then reconnects to the returning A, validated afresh, and hears its new period.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn lc_integration_the_shared_watcher_treats_a_returning_owner_as_a_new_period()
-> Result<(), Box<dyn Error>> {
    use blueroute_client::WatchSignal::{Changed, Connected, Disconnected};
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        // Watcher connect = Version 1, subscribe = 2, A's item check = 3.
        rig.script.at.store(3, Ordering::SeqCst);
        let _daemon = DaemonThread::spawn(rig.daemon(Transfer::ThroughSuccessor));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let connection = rig.client_connection.clone();
        let watcher = blueroute_client::watch(
            move || {
                let connection = connection.clone();
                async move { BlueRouteClient::from_connection(connection).await }
            },
            |_| async {
                Timer::after(Duration::from_millis(10)).await;
            },
            move |signal| {
                sink.lock().unwrap().push(signal);
                std::ops::ControlFlow::Continue(())
            },
        );
        let wait_for_signals = |expected: Vec<blueroute_client::WatchSignal>| {
            let seen = seen.clone();
            async move {
                within(Duration::from_secs(3), async {
                    while *seen.lock().unwrap() != expected {
                        Timer::after(Duration::from_millis(5)).await;
                    }
                })
                .await
                .ok_or_else(|| format!("expected {expected:?}, saw {:?}", seen.lock().unwrap()))
            }
        };
        let scenario = async {
            wait_for_signals(vec![Connected]).await?;
            emit(&rig.a, Item::Event).await?;
            wait_for(&rig.script.arrived).await;
            // Block this thread, and with it the watcher, across A→B→A and the reply.
            rig.script.go.store(true, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !rig.script.replied.load(Ordering::SeqCst) {
                if std::time::Instant::now() > deadline {
                    return Err("the daemon never replied".into());
                }
                std::thread::yield_now();
            }
            wait_for_signals(vec![Connected, Disconnected, Connected]).await?;
            emit(&rig.a, Item::State).await?;
            wait_for_signals(vec![Connected, Disconnected, Connected, Changed]).await?;
            Ok::<(), Box<dyn Error>>(())
        };
        future::or(
            async {
                watcher.await;
                Err("the watcher stopped".into())
            },
            scenario,
        )
        .await
    })
}

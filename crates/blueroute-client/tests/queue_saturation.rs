//! QR-BASE / QR-QUEUE regressions (docs/CODE_REVIEW_QUEUE_RECOVERY_TODO.md, RLC-01, QUEUE-1..6).
//!
//! zbus 5.19 reads one connection's messages in order and hands each to every matching stream
//! before it reads the next; a full stream (64 entries by default) stops the reader, so method
//! replies behind it are never read. On f957d80 a subscription paused after its item's Version
//! reply, while the owner releases and reacquires the name 40 times (80 notices), resumes into the
//! final `GetNameOwner` and waits behind its own unread ownership queue: no OwnerLost, and other
//! calls on that connection stall too. The same happens to a burst of the owner's signals queued
//! ahead of a verification reply.
//!
//! Ordering is established on the daemon side, never through the client's (possibly blocked)
//! reader: each release and reacquire is the daemon's own round trip with the broker, and the
//! broker has queued every notice for the client before it answers. Bursts are finite. Needs a
//! private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test queue_saturation --locked -- --ignored`
use std::error::Error;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_io::Timer;
use blueroute_client::{BlueRouteClient, Change, ClientError, EventSubscription};
use blueroute_core::HealthLevel;
use blueroute_protocol::{
    ApiVersion, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event, encode_event,
};
use futures_lite::{StreamExt, future};
use zbus::message::Type;
use zbus::{Connection, MessageStream};

/// The tests share the daemon's well-known name on one private bus.
static BUS: Mutex<()> = Mutex::new(());

const COMPATIBLE: (u16, u16) = (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor);
/// QUEUE-2: the bound for an actively polled consumer, measured from its resumption.
const BOUND: Duration = Duration::from_secs(2);
/// zbus's default per-stream queue capacity, which the bursts are sized against.
const ZBUS_QUEUE: usize = 64;

/// What the scripted daemon does around one Version call.
#[derive(Default)]
struct Script {
    version_calls: AtomicUsize,
    /// The Version call (1-based) that is scripted.
    at: AtomicUsize,
    /// Set when that call has arrived; the daemon then waits for `go`.
    arrived: AtomicBool,
    go: AtomicBool,
    /// Owner signals emitted ahead of the scripted reply.
    signals_before_reply: AtomicUsize,
    /// Release/reacquire cycles after the scripted reply.
    cycles_after_reply: AtomicUsize,
    /// Set once the reply is sent and every scripted broker round trip has completed.
    done: AtomicBool,
}

/// A raw message loop owning the daemon's name, so a test knows exactly when a reply was sent.
struct Daemon {
    connection: Connection,
    script: Arc<Script>,
}

impl Daemon {
    async fn serve(self) -> Result<(), Box<dyn Error>> {
        let mut calls = MessageStream::from(&self.connection);
        while let Some(message) = calls.next().await {
            let message = message?;
            let header = message.header();
            if message.message_type() != Type::MethodCall
                || header.interface().map(|name| name.as_str()) != Some(DBUS_INTERFACE_NAME)
                || header.member().map(|name| name.as_str()) != Some("Version")
            {
                continue;
            }
            let call = self.script.version_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let scripted = self.script.at.load(Ordering::SeqCst) == call;
            if scripted {
                self.script.arrived.store(true, Ordering::SeqCst);
                wait_for(&self.script.go).await;
                for _ in 0..self.script.signals_before_reply.load(Ordering::SeqCst) {
                    emit(&self.connection, Item::Event).await?;
                }
            }
            let reply = zbus::Message::method_return(&header)?.build(&COMPATIBLE)?;
            self.connection.send(&reply).await?;
            if scripted {
                for _ in 0..self.script.cycles_after_reply.load(Ordering::SeqCst) {
                    self.connection.release_name(DBUS_SERVICE_NAME).await?;
                    take_name(&self.connection).await?;
                }
                self.script.done.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }
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

/// Which kind of item the owner emits.
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

/// Takes the daemon's name, failing instead of silently queueing behind a lingering owner.
async fn take_name(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let reply = connection
        .request_name_with_flags(
            DBUS_SERVICE_NAME,
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        )
        .await?;
    if reply != zbus::fdo::RequestNameReply::PrimaryOwner {
        return Err(format!("could not take the name: {reply:?}").into());
    }
    Ok(())
}

/// One scenario's bus: daemon A owning the name, and the client's connection.
struct Rig {
    a: Connection,
    client_connection: Connection,
    script: Arc<Script>,
}

async fn rig() -> Result<Rig, Box<dyn Error>> {
    let mut a = Connection::session().await?;
    // The fake daemon's own NameAcquired/NameLost signals pile up while it runs its cycles;
    // give its unfiltered queue room for them so the fixture never stalls itself.
    a.set_max_queued(1024);
    take_name(&a).await?;
    let client_connection = Connection::session().await?;
    Ok(Rig {
        a,
        client_connection,
        script: Arc::default(),
    })
}

/// Runs `scenario` while the scripted daemon serves; the daemon stops with the scenario.
async fn with_daemon(
    rig: &Rig,
    scenario: impl Future<Output = Result<(), Box<dyn Error>>>,
) -> Result<(), Box<dyn Error>> {
    let daemon = Daemon {
        connection: rig.a.clone(),
        script: rig.script.clone(),
    };
    future::or(
        async {
            daemon.serve().await?;
            Err("the daemon stopped".into())
        },
        scenario,
    )
    .await
}

/// A broker round trip on the client's connection: it completes only while that connection's
/// reader is making progress (QUEUE-4/QR-QUEUE-06).
async fn second_call_completes(connection: &Connection) -> Result<(), Box<dyn Error>> {
    match within(BOUND, async {
        zbus::fdo::DBusProxy::new(connection).await?.get_id().await
    })
    .await
    {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => Err(format!("the second call failed: {error}").into()),
        None => Err("a second call on the shared connection stalled".into()),
    }
}

async fn subscribe(client: &BlueRouteClient, item: Item) -> Result<EventSubscription, ClientError> {
    match item {
        Item::Event => client.events().await,
        Item::State => client.changes().await,
    }
}

/// What a subscription is left holding after the burst.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Keep {
    /// Keep the retired handle alive while checking the shared connection.
    Retained,
    /// Drop it first.
    Dropped,
}

/// RLC-01 / QR-BASE-03 / QR-QUEUE-05/06: the owner emits `item`; inside that item's Version
/// check it replies and then releases and reacquires the name `cycles` times. The consumer stays
/// paused until all of that is done, then must see OwnerLost within the bound, before any
/// obsolete item, and the shared connection must keep working.
fn burst_after_the_reply(item: Item, cycles: usize, keep: Keep) -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        with_daemon(&rig, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = subscribe(&client, item).await?;
            // Version calls: 1 at connect, 2 when subscribing, 3 for the item's own check.
            rig.script.at.store(3, Ordering::SeqCst);
            rig.script
                .cycles_after_reply
                .store(cycles, Ordering::SeqCst);
            emit(&rig.a, item).await?;
            {
                let mut next = pin!(old.next_change());
                while !rig.script.arrived.load(Ordering::SeqCst) {
                    if let Some(early) = future::poll_once(next.as_mut()).await {
                        return Err(format!("finished before its check: {early:?}").into());
                    }
                    Timer::after(Duration::from_millis(2)).await;
                }
                // Paused while the reply goes out and the whole burst completes.
                rig.script.go.store(true, Ordering::SeqCst);
                wait_for(&rig.script.done).await;
                match within(BOUND, next).await {
                    Some(Err(ClientError::OwnerLost)) => {}
                    other => {
                        return Err(format!(
                            "{cycles} cycles after the reply: no OwnerLost within {BOUND:?} of \
                             resuming: {other:?}"
                        )
                        .into());
                    }
                }
            }
            match keep {
                Keep::Retained => {
                    second_call_completes(&rig.client_connection).await?;
                    // The retirement is kept for every later call.
                    match within(BOUND, old.next_change()).await {
                        Some(Err(ClientError::OwnerLost)) => {}
                        other => return Err(format!("not kept retired: {other:?}").into()),
                    }
                }
                Keep::Dropped => {
                    drop(old);
                    second_call_completes(&rig.client_connection).await?;
                }
            }
            // A fresh subscription binds to the current period and hears the owner.
            let mut fresh = subscribe(&client, item).await?;
            emit(&rig.a, item).await?;
            match within(BOUND, fresh.next_change()).await {
                Some(Ok(Change::Event(Event::HealthChanged(HealthLevel::Degraded))))
                | Some(Ok(Change::State { revision: 7 })) => Ok(()),
                other => Err(format!("the fresh subscription missed the owner: {other:?}").into()),
            }
        })
        .await
    })
}

/// Below capacity (40 notices): the control, which retires on f957d80 too.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_burst_below_capacity_retires_event() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::Event, 20, Keep::Retained)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_burst_below_capacity_retires_state() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::State, 20, Keep::Retained)
}

/// Exactly at capacity: 64 notices fill the queue without blocking the reader.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_burst_at_capacity_retires_event() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::Event, ZBUS_QUEUE / 2, Keep::Retained)
}

/// One cycle past capacity (66 notices).
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_burst_just_above_capacity_retires_state() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::State, ZBUS_QUEUE / 2 + 1, Keep::Retained)
}

/// The review's reproduction: 40 cycles, 80 notices, retired handle kept alive.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_forty_cycles_after_the_reply_retire_event() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::Event, 40, Keep::Retained)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_forty_cycles_after_the_reply_retire_state() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::State, 40, Keep::Retained)
}

/// The same reproduction with the handle dropped before the second call.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_forty_cycles_then_drop_keep_the_connection_working() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::Event, 40, Keep::Dropped)
}

/// QUEUE-1 signal selection: `count` owner signals are queued ahead of the item's Version reply
/// while the consumer is actively polled. Returns what the first call produced.
fn signals_ahead_of_the_reply(count: usize) -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        with_daemon(&rig, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut subscription = client.events().await?;
            rig.script.at.store(3, Ordering::SeqCst);
            rig.script
                .signals_before_reply
                .store(count, Ordering::SeqCst);
            rig.script.go.store(true, Ordering::SeqCst);
            emit(&rig.a, Item::Event).await?;
            let mut delivered = 0;
            let outcome = loop {
                match within(BOUND, subscription.next_event()).await {
                    Some(Ok(_)) => {
                        delivered += 1;
                        if delivered == count + 1 {
                            break Ok(());
                        }
                    }
                    Some(Err(error)) => break Err(error),
                    None => {
                        return Err(format!(
                            "{count} signals ahead of a reply: stalled after {delivered} items"
                        )
                        .into());
                    }
                }
            };
            second_call_completes(&rig.client_connection).await?;
            check_signal_outcome(count, delivered, outcome)
        })
        .await
    })
}

/// Within the subscription's bounded backlog every item is delivered; beyond it the subscription
/// ends with a typed error instead of stalling the connection (QUEUE-4: bounded buffering).
fn check_signal_outcome(
    count: usize,
    delivered: usize,
    outcome: Result<(), ClientError>,
) -> Result<(), Box<dyn Error>> {
    match outcome {
        Ok(()) if count <= 2 * ZBUS_QUEUE => Ok(()),
        Err(error) if count > 2 * ZBUS_QUEUE && delivered == 0 => {
            if matches!(
                error,
                ClientError::OwnerLost | ClientError::EventStreamClosed
            ) {
                return Err(format!("an overflow was reported as {error:?}").into());
            }
            Ok(())
        }
        other => Err(format!("{count} signals: {delivered} delivered, then {other:?}").into()),
    }
}

/// One and a half zbus queues ahead of the reply (more than the reader can hold while the
/// subscription is not draining): all of them must arrive, each within the bound.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_signals_queued_ahead_of_a_reply_are_all_delivered() -> Result<(), Box<dyn Error>> {
    signals_ahead_of_the_reply(ZBUS_QUEUE + ZBUS_QUEUE / 2)
}

/// Far beyond the backlog: a typed overflow ends the subscription, and the connection works.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_signal_flood_ends_the_subscription_without_stalling() -> Result<(), Box<dyn Error>> {
    signals_ahead_of_the_reply(5 * ZBUS_QUEUE)
}

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
use blueroute_client::{
    BlueRouteClient, Change, ClientError, EventSubscription, SUBSCRIPTION_BACKLOG,
};
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
    /// Owner signals emitted ahead of the scripted reply, and whether they are `StateChanged`.
    signals_before_reply: AtomicUsize,
    state_signals: AtomicBool,
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
                let item = if self.script.state_signals.load(Ordering::SeqCst) {
                    Item::State
                } else {
                    Item::Event
                };
                for _ in 0..self.script.signals_before_reply.load(Ordering::SeqCst) {
                    emit(&self.connection, item).await?;
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

/// Another 40 release/reacquire cycles and 100 signals, more than any zbus queue holds.
async fn churn_after_retirement(rig: &Rig) -> Result<(), Box<dyn Error>> {
    for _ in 0..40 {
        rig.a.release_name(DBUS_SERVICE_NAME).await?;
        take_name(&rig.a).await?;
    }
    for _ in 0..100 {
        emit(&rig.a, Item::Event).await?;
        emit(&rig.a, Item::State).await?;
    }
    Ok(())
}

async fn subscribe(client: &BlueRouteClient, item: Item) -> Result<EventSubscription, ClientError> {
    match item {
        Item::Event => client.events().await,
        Item::State => client.changes().await,
    }
}

/// Polls `next` only until the scripted Version call reaches the daemon, then lets the daemon
/// reply and run its whole burst while `next` stays paused.
async fn pause_across_the_burst<T: std::fmt::Debug>(
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
    wait_for(&script.done).await;
    Ok(())
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
                // Paused while the reply goes out and the whole burst completes.
                pause_across_the_burst(&rig.script, next.as_mut()).await?;
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
                    // More churn and more signals after retirement, with the handle still alive
                    // and never polled again: it must hold none of them back (QUEUE-4).
                    churn_after_retirement(&rig).await?;
                    second_call_completes(&rig.client_connection).await?;
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

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_burst_at_capacity_retires_state() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::State, ZBUS_QUEUE / 2, Keep::Retained)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_burst_just_above_capacity_retires_event() -> Result<(), Box<dyn Error>> {
    burst_after_the_reply(Item::Event, ZBUS_QUEUE / 2 + 1, Keep::Retained)
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

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_forty_cycles_then_drop_keep_the_connection_working_state() -> Result<(), Box<dyn Error>>
{
    burst_after_the_reply(Item::State, 40, Keep::Dropped)
}

/// QUEUE-1 signal selection: `count` owner signals are queued ahead of the item's Version reply
/// while the consumer is actively polled; every one must be delivered in order or the
/// subscription must end with the typed overflow, and the connection must keep working.
fn signals_ahead_of_the_reply(item: Item, count: usize) -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        with_daemon(&rig, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut subscription = subscribe(&client, item).await?;
            rig.script.at.store(3, Ordering::SeqCst);
            rig.script
                .signals_before_reply
                .store(count, Ordering::SeqCst);
            rig.script
                .state_signals
                .store(matches!(item, Item::State), Ordering::SeqCst);
            rig.script.go.store(true, Ordering::SeqCst);
            emit(&rig.a, item).await?;
            let mut delivered = 0;
            let outcome = loop {
                match within(BOUND, subscription.next_change()).await {
                    Some(Ok(Change::Event(Event::HealthChanged(HealthLevel::Degraded))))
                    | Some(Ok(Change::State { revision: 7 })) => {
                        delivered += 1;
                        if delivered == count + 1 {
                            break Ok(());
                        }
                    }
                    Some(Ok(other)) => return Err(format!("an unexpected item: {other:?}").into()),
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

/// Within the subscription's bounded backlog every item is delivered; well beyond it the
/// subscription ends with the typed overflow instead of stalling the connection (QUEUE-4:
/// bounded buffering). Between the two the outcome depends on how much the reader had queued.
fn check_signal_outcome(
    count: usize,
    delivered: usize,
    outcome: Result<(), ClientError>,
) -> Result<(), Box<dyn Error>> {
    match outcome {
        Ok(()) if count <= SUBSCRIPTION_BACKLOG => Ok(()),
        Err(ClientError::SubscriptionLagged { limit })
            if count > SUBSCRIPTION_BACKLOG + ZBUS_QUEUE
                && limit == SUBSCRIPTION_BACKLOG
                && delivered == 0 =>
        {
            Ok(())
        }
        other => Err(format!("{count} signals: {delivered} delivered, then {other:?}").into()),
    }
}

/// One and a half zbus queues ahead of the reply (more than the reader can hold while the
/// subscription is not draining): all of them must arrive, each within the bound.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_events_queued_ahead_of_a_reply_are_all_delivered() -> Result<(), Box<dyn Error>> {
    signals_ahead_of_the_reply(Item::Event, ZBUS_QUEUE + ZBUS_QUEUE / 2)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_state_changes_queued_ahead_of_a_reply_are_all_delivered() -> Result<(), Box<dyn Error>>
{
    signals_ahead_of_the_reply(Item::State, ZBUS_QUEUE + ZBUS_QUEUE / 2)
}

/// Far beyond the backlog: a typed overflow ends the subscription, and the connection works.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_an_event_flood_ends_the_subscription_without_stalling() -> Result<(), Box<dyn Error>> {
    signals_ahead_of_the_reply(Item::Event, SUBSCRIPTION_BACKLOG + 2 * ZBUS_QUEUE)
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_state_change_flood_ends_the_subscription_without_stalling()
-> Result<(), Box<dyn Error>> {
    signals_ahead_of_the_reply(Item::State, SUBSCRIPTION_BACKLOG + 2 * ZBUS_QUEUE)
}

/// QR-QUEUE-06: the paused call is cancelled after the 40-cycle burst instead of resumed. The
/// next call must still report the loss within the bound, and the connection must work.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_call_cancelled_during_the_burst_loses_nothing() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        with_daemon(&rig, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            rig.script.at.store(3, Ordering::SeqCst);
            rig.script.cycles_after_reply.store(40, Ordering::SeqCst);
            emit(&rig.a, Item::Event).await?;
            {
                let mut next = pin!(old.next_change());
                pause_across_the_burst(&rig.script, next.as_mut()).await?;
                // Cancelled here: the future is dropped with its check pending.
            }
            match within(BOUND, old.next_change()).await {
                Some(Err(ClientError::OwnerLost)) => {}
                other => return Err(format!("the loss was lost by cancelling: {other:?}").into()),
            }
            second_call_completes(&rig.client_connection).await
        })
        .await
    })
}

/// QR-QUEUE-06: a retired handle and a live subscription share zbus match channels (same rules).
/// Ending, keeping and then dropping the retired one must leave the live one working.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_live_subscription_survives_a_retired_one_on_the_same_rules()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        with_daemon(&rig, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.changes().await?;
            rig.script.at.store(3, Ordering::SeqCst);
            rig.script.cycles_after_reply.store(40, Ordering::SeqCst);
            emit(&rig.a, Item::State).await?;
            {
                let mut next = pin!(old.next_change());
                pause_across_the_burst(&rig.script, next.as_mut()).await?;
                match within(BOUND, next).await {
                    Some(Err(ClientError::OwnerLost)) => {}
                    other => return Err(format!("not retired: {other:?}").into()),
                }
            }
            let mut live = client.changes().await?;
            emit(&rig.a, Item::State).await?;
            match within(BOUND, live.next_change()).await {
                Some(Ok(Change::State { revision: 7 })) => {}
                other => return Err(format!("with the retired handle kept: {other:?}").into()),
            }
            // Dropping the retired handle releases its share of the match rules only.
            drop(old);
            emit(&rig.a, Item::Event).await?;
            match within(BOUND, live.next_change()).await {
                Some(Ok(Change::Event(Event::HealthChanged(HealthLevel::Degraded)))) => {}
                other => {
                    return Err(format!("after the retired handle was dropped: {other:?}").into());
                }
            }
            second_call_completes(&rig.client_connection).await
        })
        .await
    })
}

/// QUEUE-6: the progress argument rests on pinned zbus 5.19.0 (async-broadcast 0.7.2) reader
/// behavior: one reader per connection, method replies dispatched before the reader waits for
/// room in each matching stream, 64-entry match channels shared per rule. An upgrade must
/// re-verify that argument (see docs/CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md) before this pin is
/// changed, so a lockfile change cannot silently invalidate it.
#[test]
fn qr_queue_the_transport_argument_is_pinned_to_the_verified_zbus() {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
        .expect("the workspace lockfile");
    for (name, verified) in [("zbus", "5.19.0"), ("async-broadcast", "0.7.2")] {
        let entry = format!("name = \"{name}\"\nversion = \"");
        let versions: Vec<&str> = lock
            .match_indices(&entry)
            .map(|(at, _)| {
                let rest = &lock[at + entry.len()..];
                &rest[..rest.find('"').expect("a quoted version")]
            })
            .collect();
        assert_eq!(
            versions,
            [verified],
            "{name} changed: re-verify the QR-QUEUE progress argument against its reader"
        );
    }
}

/// Runs the scripted daemon on its own thread until dropped, so the test thread can block.
struct DaemonThread {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl DaemonThread {
    fn spawn(rig: &Rig) -> Self {
        let daemon = Daemon {
            connection: rig.a.clone(),
            script: rig.script.clone(),
        };
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

/// QR-INTEGRATION-01: the RLC-01 reproduction through the production shared watcher. The test
/// blocks the watcher's executor thread from the item's Version call until the daemon has
/// replied and finished 40 release/reacquire cycles. The watcher must then report the loss within
/// the bound, pass nothing of the old period on, reconnect and hear the current period.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_integration_the_shared_watcher_reports_a_saturated_loss() -> Result<(), Box<dyn Error>> {
    use blueroute_client::WatchSignal::{Changed, Connected, Disconnected};
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        // Watcher connect = Version 1, subscribe = 2, the item's check = 3.
        rig.script.at.store(3, Ordering::SeqCst);
        rig.script.cycles_after_reply.store(40, Ordering::SeqCst);
        let _daemon = DaemonThread::spawn(&rig);
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
        let wait_for_signals = |expected: Vec<blueroute_client::WatchSignal>, bound| {
            let seen = seen.clone();
            async move {
                within(bound, async {
                    while *seen.lock().unwrap() != expected {
                        Timer::after(Duration::from_millis(5)).await;
                    }
                })
                .await
                .ok_or_else(|| format!("expected {expected:?}, saw {:?}", seen.lock().unwrap()))
            }
        };
        let scenario = async {
            wait_for_signals(vec![Connected], Duration::from_secs(3)).await?;
            emit(&rig.a, Item::State).await?;
            wait_for(&rig.script.arrived).await;
            // Block this thread, and with it the watcher, across the reply and the burst.
            rig.script.go.store(true, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !rig.script.done.load(Ordering::SeqCst) {
                if std::time::Instant::now() > deadline {
                    return Err("the daemon never finished its burst".into());
                }
                std::thread::yield_now();
            }
            // Resumed: the loss within the bound, then the reconnect on the 10 ms backoff.
            wait_for_signals(vec![Connected, Disconnected], BOUND).await?;
            wait_for_signals(
                vec![Connected, Disconnected, Connected],
                Duration::from_secs(3),
            )
            .await?;
            emit(&rig.a, Item::State).await?;
            wait_for_signals(
                vec![Connected, Disconnected, Connected, Changed],
                Duration::from_secs(2),
            )
            .await?;
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

/// A broker round trip on `connection`; its reply arrives behind every earlier reply the broker
/// sent to that connection.
async fn broker_round_trip(connection: &Connection) -> Result<(), Box<dyn Error>> {
    zbus::fdo::DBusProxy::new(connection)
        .await?
        .get_id()
        .await?;
    Ok(())
}

/// QR-REVIEW / QUEUE-3: a period end received **after** the final `GetNameOwner` reply. A bus
/// monitor shows when the client has sent that call; the client is then left unpolled while the
/// broker answers it (a later round trip on the same connection proves the reply is queued) and
/// while A releases and reacquires the name, so both notices arrive after the decision. On
/// resumption the permitted delivery must stand, the next call must report OwnerLost, and the
/// retired handle must hold nothing back.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn qr_queue_a_period_end_after_the_decision_keeps_the_delivery_and_retires_next()
-> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let rig = rig().await?;
        with_daemon(&rig, async {
            let client = BlueRouteClient::from_connection(rig.client_connection.clone()).await?;
            let mut old = client.events().await?;
            let client_name = rig
                .client_connection
                .unique_name()
                .expect("a bus connection has a unique name")
                .to_owned();
            let monitor = Connection::session().await?;
            let rule = zbus::MatchRule::builder()
                .msg_type(Type::MethodCall)
                .sender(client_name.as_str())?
                .member("GetNameOwner")?
                .build();
            zbus::fdo::MonitoringProxy::new(&monitor)
                .await?
                .become_monitor(&[rule], 0)
                .await?;
            let mut calls = MessageStream::from(&monitor);
            emit(&rig.a, Item::Event).await?;
            {
                let mut next = pin!(old.next_change());
                // Poll the client one step at a time, and after each step wait (unpolled) to see
                // whether that step sent the final check.
                let mut sent = false;
                for _ in 0..500 {
                    if let Some(early) = future::poll_once(next.as_mut()).await {
                        return Err(format!("finished before its final check: {early:?}").into());
                    }
                    if let Some(Some(call)) = within(Duration::from_millis(10), calls.next()).await
                    {
                        let call = call?;
                        if call.header().member().map(|member| member.as_str())
                            == Some("GetNameOwner")
                        {
                            sent = true;
                            break;
                        }
                    }
                }
                if !sent {
                    return Err("the final check was never sent".into());
                }
                // Unpolled: the reply is queued behind this round trip's own, then the period
                // ends and its notices are queued after both.
                broker_round_trip(&rig.client_connection).await?;
                rig.a.release_name(DBUS_SERVICE_NAME).await?;
                take_name(&rig.a).await?;
                broker_round_trip(&rig.client_connection).await?;
                match within(BOUND, next).await {
                    Some(Ok(Change::Event(Event::HealthChanged(HealthLevel::Degraded)))) => {}
                    other => {
                        return Err(format!(
                            "a post-decision end withdrew the delivery: {other:?}"
                        )
                        .into());
                    }
                }
            }
            // Retired already, before it is polled again: the handle holds nothing back through
            // more churn and signals, and the next call reports the loss.
            churn_after_retirement(&rig).await?;
            second_call_completes(&rig.client_connection).await?;
            match within(BOUND, old.next_change()).await {
                Some(Err(ClientError::OwnerLost)) => Ok(()),
                other => Err(format!("the retirement was not kept: {other:?}").into()),
            }
        })
        .await
    })
}

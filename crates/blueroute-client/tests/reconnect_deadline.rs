//! RC-BASE / RC-TEST regressions for RQR-01 (docs/CODE_REVIEW_RECONNECT_TODO.md, DEADLINE-1..6,
//! COMPAT-1..3): `BlueRouteClient::reconnect(timeout)` must finish within its budget however the
//! daemon behaves.
//!
//! On bd58ce2 the deadline was checked only after a bus error, before a fixed 100 ms sleep: a
//! held Version reply kept `reconnect(50 ms)` pending until the reply was released, and a reply
//! that arrived after the deadline was still accepted. A scripted daemon (a raw message loop on
//! its own connection) holds and releases its Version replies on the test's command, so each
//! scenario knows exactly when a reply was sent. A watchdog bounds every wait on the client; it
//! only turns a stalled client into a test failure and is never the client's timeout.
//!
//! Needs a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test reconnect_deadline --locked -- --ignored`
use std::error::Error;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_client::{BlueRouteClient, ClientError, ClientErrorKind};
use blueroute_core::HealthLevel;
use blueroute_protocol::{
    ApiVersion, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event, encode_event,
};
use futures_lite::{StreamExt, future};
use zbus::message::Type;
use zbus::{Connection, MessageStream};

/// The tests share the daemon's well-known name on one private bus.
static BUS: Mutex<()> = Mutex::new(());

/// The client's own version: what `from_connection` records.
const INITIAL: ApiVersion = ApiVersion::CURRENT;
/// A compatible version distinguishable from `INITIAL`, so an update is observable.
const NEWER: ApiVersion = ApiVersion::new(ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor + 1);
/// An incompatible version.
const INCOMPATIBLE: ApiVersion = ApiVersion::new(ApiVersion::CURRENT.major + 1, 0);
/// The review's budget.
const BUDGET: Duration = Duration::from_millis(50);
/// Scheduler tolerance on top of a budget. The review saw the 50 ms operation still pending at
/// 250 ms; the bound accepted here (200 ms) stays below that.
const TOLERANCE: Duration = Duration::from_millis(150);
/// Shorter than the client's 100 ms retry interval.
const SHORT: Duration = Duration::from_millis(20);
/// The client's retry interval, which a short budget must not incur in full.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// Fixture watchdog: long enough to expose the original failure (pending until released).
const WATCHDOG: Duration = Duration::from_secs(2);

fn pack(version: ApiVersion) -> u32 {
    (u32::from(version.major) << 16) | u32::from(version.minor)
}

fn unpack(packed: u32) -> (u16, u16) {
    // Both halves fit by construction.
    ((packed >> 16) as u16, (packed & 0xffff) as u16)
}

/// What the scripted daemon does with its calls.
struct Script {
    version_calls: AtomicUsize,
    request_calls: AtomicUsize,
    /// The Version call (1-based) to hold; 0 holds none.
    hold_at: AtomicUsize,
    /// Set when the held call has arrived; the daemon then waits for `go`.
    arrived: AtomicBool,
    go: AtomicBool,
    /// Set once the held call's reply has been sent.
    sent: AtomicBool,
    /// The reply to the held call.
    held_reply: AtomicU32,
    /// The reply to every other Version call.
    reply: AtomicU32,
    /// Receive ordinary Requests without ever replying (a reply lost after delivery).
    swallow_requests: AtomicBool,
}

impl Script {
    fn new(reply: ApiVersion) -> Arc<Self> {
        Arc::new(Self {
            version_calls: AtomicUsize::new(0),
            request_calls: AtomicUsize::new(0),
            hold_at: AtomicUsize::new(0),
            arrived: AtomicBool::new(false),
            go: AtomicBool::new(false),
            sent: AtomicBool::new(false),
            held_reply: AtomicU32::new(pack(reply)),
            reply: AtomicU32::new(pack(reply)),
            swallow_requests: AtomicBool::new(false),
        })
    }

    fn reply_with(&self, version: ApiVersion) {
        self.reply.store(pack(version), Ordering::SeqCst);
    }

    fn hold(&self, call: usize, reply: ApiVersion) {
        self.held_reply.store(pack(reply), Ordering::SeqCst);
        self.hold_at.store(call, Ordering::SeqCst);
    }

    fn version_calls(&self) -> usize {
        self.version_calls.load(Ordering::SeqCst)
    }

    fn request_calls(&self) -> usize {
        self.request_calls.load(Ordering::SeqCst)
    }
}

/// Serves `script` on `connection`, which owns the daemon's name; never returns on its own.
async fn serve(connection: Connection, script: Arc<Script>) -> Result<(), Box<dyn Error>> {
    let mut calls = MessageStream::from(&connection);
    while let Some(message) = calls.next().await {
        let Ok(message) = message else { break };
        let header = message.header();
        if message.message_type() != Type::MethodCall
            || header.interface().map(|name| name.as_str()) != Some(DBUS_INTERFACE_NAME)
        {
            continue;
        }
        match header.member().map(|name| name.as_str()) {
            Some("Version") => {
                let call = script.version_calls.fetch_add(1, Ordering::SeqCst) + 1;
                let held = script.hold_at.load(Ordering::SeqCst) == call;
                let reply = if held {
                    script.arrived.store(true, Ordering::SeqCst);
                    wait_for(&script.go).await;
                    script.held_reply.load(Ordering::SeqCst)
                } else {
                    script.reply.load(Ordering::SeqCst)
                };
                let reply = zbus::Message::method_return(&header)?.build(&unpack(reply))?;
                connection.send(&reply).await?;
                if held {
                    script.sent.store(true, Ordering::SeqCst);
                }
            }
            Some("Request") => {
                script.request_calls.fetch_add(1, Ordering::SeqCst);
                if !script.swallow_requests.load(Ordering::SeqCst) {
                    let error = zbus::Message::error(&header, "org.freedesktop.DBus.Error.Failed")?
                        .build(&"the deadline fixture serves no ordinary requests")?;
                    connection.send(&error).await?;
                }
            }
            _ => {}
        }
    }
    // The connection was closed: stay silent, like a daemon that went away.
    future::pending::<()>().await;
    Ok(())
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

/// A connection owning the daemon's name, with its script.
async fn daemon(reply: ApiVersion) -> Result<(Connection, Arc<Script>), Box<dyn Error>> {
    let connection = Connection::session().await?;
    take_name(&connection).await?;
    Ok((connection, Script::new(reply)))
}

/// Runs `scenario` while daemon A (owning the name, answering `INITIAL`) serves, with a client
/// built over a fresh connection. A's Version call 1 is the client's construction.
fn scenario<F, Fut>(body: F) -> Result<(), Box<dyn Error>>
where
    F: FnOnce(Connection, Arc<Script>, BlueRouteClient, Connection) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let (a, script) = daemon(INITIAL).await?;
        future::or(
            async {
                serve(a.clone(), script.clone()).await?;
                Err("daemon A stopped".into())
            },
            async {
                let client_connection = Connection::session().await?;
                let client = BlueRouteClient::from_connection(client_connection.clone()).await?;
                assert_eq!(client.server_version(), INITIAL);
                let outcome = futures_lite::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                    body(a.clone(), script.clone(), client, client_connection),
                ))
                .await;
                // Leave the name free for the next scenario whatever happened, a failed
                // assertion included, so one failure does not cascade into the next scenario.
                let _ = a.release_name(DBUS_SERVICE_NAME).await;
                outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            },
        )
        .await
    })
}

/// Requires the existing typed timeout for `budget`, Unavailable, and returns its diagnostic.
fn expect_timeout(
    outcome: Option<Result<(), ClientError>>,
    budget: Duration,
) -> Result<String, Box<dyn Error>> {
    match outcome {
        Some(Err(error @ ClientError::ReconnectTimeout { timeout, .. })) if timeout == budget => {
            if error.kind() != ClientErrorKind::Unavailable {
                return Err(format!("a timeout must classify as Unavailable: {error:?}").into());
            }
            let ClientError::ReconnectTimeout { last_error, .. } = error else {
                unreachable!()
            };
            Ok(last_error)
        }
        Some(other) => Err(format!("expected ReconnectTimeout({budget:?}): {other:?}").into()),
        None => Err(format!("reconnect({budget:?}) was still pending after {WATCHDOG:?}").into()),
    }
}

fn expect_within(elapsed: Duration, bound: Duration, what: &str) -> Result<(), Box<dyn Error>> {
    if elapsed > bound {
        return Err(format!("{what} took {elapsed:?}, more than {bound:?}").into());
    }
    eprintln!("{what}: {elapsed:?} (bound {bound:?})");
    Ok(())
}

/// Version calls a held one: resolves when the client's Version call has reached the daemon.
async fn poll_until_held<T: std::fmt::Debug>(
    script: &Script,
    mut operation: std::pin::Pin<&mut impl Future<Output = T>>,
) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    while !script.arrived.load(Ordering::SeqCst) {
        if let Some(early) = future::poll_once(operation.as_mut()).await {
            return Err(format!("finished before its Version call was held: {early:?}").into());
        }
        if started.elapsed() > WATCHDOG {
            return Err("the Version call never reached the daemon".into());
        }
        Timer::after(Duration::from_millis(1)).await;
    }
    Ok(())
}

/// A broker round trip on `connection`; it completes only while that connection's reader works.
async fn round_trip(connection: &Connection) -> Result<(), Box<dyn Error>> {
    match within(WATCHDOG, async {
        zbus::fdo::DBusProxy::new(connection).await?.get_id().await
    })
    .await
    {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => {
            Err(format!("a round trip on the client connection failed: {error}").into())
        }
        None => Err("a round trip on the client connection stalled".into()),
    }
}

/// Counts the client's `GetNameOwner` calls through a bus monitor.
struct Lookups {
    _monitor: Connection,
    calls: MessageStream,
    seen: usize,
}

impl Lookups {
    async fn watch(client: &Connection) -> Result<Self, Box<dyn Error>> {
        let sender = client
            .unique_name()
            .ok_or("a bus connection has a unique name")?
            .to_owned();
        let monitor = Connection::session().await?;
        let rule = zbus::MatchRule::builder()
            .msg_type(Type::MethodCall)
            .sender(sender.as_str())?
            .member("GetNameOwner")?
            .build();
        zbus::fdo::MonitoringProxy::new(&monitor)
            .await?
            .become_monitor(&[rule], 0)
            .await?;
        let calls = MessageStream::from(&monitor);
        Ok(Self {
            _monitor: monitor,
            calls,
            seen: 0,
        })
    }

    /// Every lookup seen so far, after `quiet` passes with no new one.
    async fn settle(&mut self, quiet: Duration) -> Result<usize, Box<dyn Error>> {
        while let Some(Some(call)) = within(quiet, self.calls.next()).await {
            let call = call?;
            if call.header().member().map(|member| member.as_str()) == Some("GetNameOwner") {
                self.seen += 1;
            }
        }
        Ok(self.seen)
    }
}

/// RQR-01 / RC-BASE-03 / RC-TEST-01: the review's reproduction. The first Version reply is
/// immediate (construction); reconnect's is held. `reconnect(50 ms)` must report the typed
/// timeout while the reply is still held, the late reply must change nothing, and the
/// connection must serve a fresh reconnect afterwards.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_a_held_version_reply_times_out_within_the_budget() -> Result<(), Box<dyn Error>> {
    scenario(|_a, script, mut client, connection| async move {
        script.reply_with(NEWER);
        script.hold(2, NEWER);
        let started = Instant::now();
        let outcome = within(WATCHDOG, client.reconnect(BUDGET)).await;
        let elapsed = started.elapsed();
        let still_held =
            script.arrived.load(Ordering::SeqCst) && !script.sent.load(Ordering::SeqCst);
        // Release the reply whatever happened, so the fixture never hangs.
        script.go.store(true, Ordering::SeqCst);
        let last_error = expect_timeout(outcome, BUDGET)?;
        expect_within(
            elapsed,
            BUDGET + TOLERANCE,
            "reconnect(50 ms) with a held Version",
        )?;
        if !still_held {
            return Err("the timeout must be reported while the Version reply is held".into());
        }
        if last_error.contains("D-Bus operation failed") {
            return Err(format!("a pending negotiation is not a bus failure: {last_error}").into());
        }
        // The late reply arrives on a connection that keeps working, and changes nothing.
        wait_for(&script.sent).await;
        round_trip(&connection).await?;
        assert_eq!(
            client.server_version(),
            INITIAL,
            "a late reply updated the version"
        );
        // A fresh operation negotiates afresh.
        match within(WATCHDOG, client.reconnect(Duration::from_secs(1))).await {
            Some(Ok(())) => {}
            other => return Err(format!("a fresh reconnect failed: {other:?}").into()),
        }
        assert_eq!(client.server_version(), NEWER);
        assert_eq!(
            script.request_calls(),
            0,
            "reconnect sent an ordinary Request"
        );
        Ok(())
    })
}

/// RC-BASE-03 control / RC-TEST-05: a timely compatible reply succeeds and records its version.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_a_timely_compatible_reply_succeeds_and_records_its_version() -> Result<(), Box<dyn Error>> {
    scenario(|_a, script, mut client, _connection| async move {
        script.reply_with(NEWER);
        // DEADLINE-1: created, then left unpolled for longer than its budget. The budget starts
        // on the first poll, so the operation still has all of it.
        let unpolled = client.reconnect(BUDGET);
        Timer::after(BUDGET * 2).await;
        match within(WATCHDOG, unpolled).await {
            Some(Ok(())) => {}
            other => {
                return Err(format!("the budget started before the first poll: {other:?}").into());
            }
        }
        let started = Instant::now();
        match within(WATCHDOG, client.reconnect(Duration::from_secs(1))).await {
            Some(Ok(())) => {}
            other => return Err(format!("a timely reply must succeed: {other:?}").into()),
        }
        expect_within(
            started.elapsed(),
            Duration::from_secs(1),
            "timely reconnect",
        )?;
        assert_eq!(client.server_version(), NEWER);
        assert_eq!(script.version_calls(), 3);
        assert_eq!(
            script.request_calls(),
            0,
            "reconnect sent an ordinary Request"
        );
        Ok(())
    })
}

/// RC-TEST-05: a timely incompatible reply fails at once, without retries or Requests.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_a_timely_incompatible_reply_fails_at_once() -> Result<(), Box<dyn Error>> {
    scenario(|_a, script, mut client, _connection| async move {
        script.reply_with(INCOMPATIBLE);
        let started = Instant::now();
        match within(WATCHDOG, client.reconnect(Duration::from_secs(1))).await {
            Some(Err(ClientError::IncompatibleVersion {
                client: mine,
                server,
            })) if mine == ApiVersion::CURRENT && server == INCOMPATIBLE => {}
            other => return Err(format!("expected IncompatibleVersion: {other:?}").into()),
        }
        expect_within(
            started.elapsed(),
            Duration::from_millis(500),
            "timely incompatibility",
        )?;
        assert_eq!(
            script.version_calls(),
            2,
            "an incompatible reply was retried"
        );
        assert_eq!(client.server_version(), INITIAL);
        assert_eq!(
            script.request_calls(),
            0,
            "reconnect sent an ordinary Request"
        );
        Ok(())
    })
}

/// RC-TEST-03 / DEADLINE-6: a zero budget times out at once, sends no lookup or Version call,
/// and leaves the client unchanged.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_a_zero_budget_times_out_without_negotiating() -> Result<(), Box<dyn Error>> {
    scenario(|_a, script, mut client, connection| async move {
        script.reply_with(NEWER);
        let mut lookups = Lookups::watch(&connection).await?;
        let started = Instant::now();
        let outcome = within(WATCHDOG, client.reconnect(Duration::ZERO)).await;
        let elapsed = started.elapsed();
        expect_timeout(outcome, Duration::ZERO)?;
        expect_within(elapsed, TOLERANCE, "reconnect(0)")?;
        round_trip(&connection).await?;
        assert_eq!(
            lookups.settle(Duration::from_millis(200)).await?,
            0,
            "a lookup was sent"
        );
        assert_eq!(script.version_calls(), 1, "a Version call was sent");
        assert_eq!(client.server_version(), INITIAL);
        Ok(())
    })
}

/// RC-TEST-03 / DEADLINE-3: with the service absent, a budget shorter than the retry interval
/// caps the wait, keeps the bus diagnostic, and sends no lookup after expiry.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_an_absent_service_with_a_short_budget_caps_the_wait() -> Result<(), Box<dyn Error>> {
    scenario(|a, _script, mut client, connection| async move {
        a.release_name(DBUS_SERVICE_NAME).await?;
        let mut lookups = Lookups::watch(&connection).await?;
        let started = Instant::now();
        let outcome = within(WATCHDOG, client.reconnect(SHORT)).await;
        let elapsed = started.elapsed();
        let last_error = expect_timeout(outcome, SHORT)?;
        // Strictly below one full retry interval: the sleep was capped.
        expect_within(
            elapsed,
            RETRY_INTERVAL - Duration::from_millis(5),
            "reconnect(20 ms) with the service absent",
        )?;
        if !last_error.contains("NameHasNoOwner") {
            return Err(format!("the bus diagnostic was lost: {last_error}").into());
        }
        let attempts = lookups.settle(Duration::from_millis(300)).await?;
        assert_eq!(attempts, 1, "a lookup was sent after the budget expired");
        assert_eq!(client.server_version(), INITIAL);
        // The name was released by the scenario: take it back so cleanup is uniform.
        take_name(&a).await?;
        Ok(())
    })
}

/// The held reply arrives after the deadline while the client is not polled; on resumption
/// the reply and the timer are both ready and the timeout must win.
async fn late_reply_loses(
    script: Arc<Script>,
    mut client: BlueRouteClient,
    connection: Connection,
    late: ApiVersion,
) -> Result<(), Box<dyn Error>> {
    script.reply_with(NEWER);
    script.hold(2, late);
    // Sees every message the client's reader dispatches, so the test knows the reply is in.
    // zbus 5.19's reader completes a pending call's reply before it broadcasts the message to
    // streams (socket_reader.rs), so once this stream has the reply, the reconnect's call is
    // ready too and the single poll below sees both it and the deadline ready. Were that order
    // ever reversed the poll would see only the deadline: still a pass, but no longer this
    // precedence check, which the seam test `rc_an_outcome_decided_after_the_deadline_loses_to_it`
    // covers independently. `queue_saturation` pins the zbus version this relies on.
    let mut seen = MessageStream::from(&connection);
    let outcome = {
        let mut operation = pin!(client.reconnect(BUDGET));
        poll_until_held(&script, operation.as_mut()).await?;
        // Unpolled past the deadline, then the reply is released and read by the client.
        Timer::after(BUDGET + Duration::from_millis(50)).await;
        script.go.store(true, Ordering::SeqCst);
        wait_for(&script.sent).await;
        let expected = unpack(pack(late));
        let arrived = within(WATCHDOG, async {
            while let Some(message) = seen.next().await {
                let Ok(message) = message else { continue };
                if message.message_type() == Type::MethodReturn
                    && message.body().deserialize::<(u16, u16)>().ok() == Some(expected)
                {
                    return true;
                }
            }
            false
        })
        .await;
        if arrived != Some(true) {
            return Err("the late reply never reached the client".into());
        }
        drop(seen);
        // One poll: both the reply and the deadline are ready.
        future::poll_once(operation.as_mut()).await
    };
    match outcome {
        Some(Err(ClientError::ReconnectTimeout { timeout, .. })) if timeout == BUDGET => {}
        other => {
            return Err(format!(
                "a reply ready after the deadline ({late}) beat the timeout: {other:?}"
            )
            .into());
        }
    }
    assert_eq!(
        client.server_version(),
        INITIAL,
        "a late reply updated the version"
    );
    round_trip(&connection).await?;
    match within(WATCHDOG, client.reconnect(Duration::from_secs(1))).await {
        Some(Ok(())) => {}
        other => return Err(format!("a fresh reconnect failed: {other:?}").into()),
    }
    assert_eq!(client.server_version(), NEWER);
    assert_eq!(
        script.request_calls(),
        0,
        "reconnect sent an ordinary Request"
    );
    Ok(())
}

/// RC-TEST-04 / DEADLINE-4: expiry wins over a compatible reply ready at the same time.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_expiry_wins_over_a_late_compatible_reply() -> Result<(), Box<dyn Error>> {
    scenario(|_a, script, client, connection| late_reply_loses(script, client, connection, NEWER))
}

/// RC-TEST-04 / DEADLINE-4: expiry wins over an incompatible reply ready at the same time.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_expiry_wins_over_a_late_incompatible_reply() -> Result<(), Box<dyn Error>> {
    scenario(|_a, script, client, connection| {
        late_reply_loses(script, client, connection, INCOMPATIBLE)
    })
}

/// RC-TEST-05: a normal restart. A goes away; B takes the name while reconnect is retrying.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_a_restarted_daemon_is_reached_within_the_budget() -> Result<(), Box<dyn Error>> {
    scenario(|a, _script, mut client, _connection| async move {
        a.release_name(DBUS_SERVICE_NAME).await?;
        let (b, b_script) = daemon(NEWER).await?;
        b.release_name(DBUS_SERVICE_NAME).await?;
        let outcome = future::or(
            async {
                serve(b.clone(), b_script.clone()).await?;
                Err("daemon B stopped".into())
            },
            async {
                let restart = async {
                    Timer::after(Duration::from_millis(150)).await;
                    take_name(&b).await
                };
                let (restarted, reconnected) = future::zip(
                    restart,
                    within(WATCHDOG, client.reconnect(Duration::from_secs(3))),
                )
                .await;
                restarted?;
                match reconnected {
                    Some(Ok(())) => Ok::<(), Box<dyn Error>>(()),
                    other => Err(format!("the restart was not reached: {other:?}").into()),
                }
            },
        )
        .await;
        let _ = b.release_name(DBUS_SERVICE_NAME).await;
        take_name(&a).await?;
        outcome?;
        assert_eq!(client.server_version(), NEWER);
        assert_eq!(
            b_script.request_calls(),
            0,
            "reconnect sent an ordinary Request"
        );
        Ok(())
    })
}

/// RC-TEST-05 / COMPAT-1: a command delivered to A whose reply is lost is reported, not
/// replayed: neither reconnect nor the next operation sends it to B.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_a_command_whose_reply_was_lost_is_never_replayed() -> Result<(), Box<dyn Error>> {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    future::block_on(async {
        let (a, a_script) = daemon(INITIAL).await?;
        a_script.swallow_requests.store(true, Ordering::SeqCst);
        let a_served = a.clone();
        let a_daemon = a_script.clone();
        // B connects now and takes the name only once A is gone.
        let b = Connection::session().await?;
        let b_script = Script::new(NEWER);
        let b_served = b.clone();
        let b_daemon = b_script.clone();
        let daemons = async move {
            let (a_stopped, b_stopped) =
                future::zip(serve(a_served, a_daemon), serve(b_served, b_daemon)).await;
            a_stopped?;
            b_stopped?;
            Err::<(), Box<dyn Error>>("the daemons stopped".into())
        };
        future::or(daemons, async {
            let connection = Connection::session().await?;
            let mut client = BlueRouteClient::from_connection(connection.clone()).await?;
            let status = {
                let mut status = pin!(client.status());
                let started = Instant::now();
                while a_script.request_calls() == 0 {
                    if let Some(early) = future::poll_once(status.as_mut()).await {
                        return Err(format!("status finished before delivery: {early:?}").into());
                    }
                    if started.elapsed() > WATCHDOG {
                        return Err("the command never reached A".into());
                    }
                    Timer::after(Duration::from_millis(1)).await;
                }
                // Delivered; A goes away without replying and B takes over.
                a.clone().close().await?;
                take_name(&b).await?;
                within(WATCHDOG, status).await
            };
            match status {
                Some(Err(_)) => {}
                other => return Err(format!("a lost reply must be reported: {other:?}").into()),
            }
            match within(WATCHDOG, client.reconnect(Duration::from_secs(1))).await {
                Some(Ok(())) => {}
                other => return Err(format!("B was not reached: {other:?}").into()),
            }
            assert_eq!(client.server_version(), NEWER);
            round_trip(&connection).await?;
            assert_eq!(a_script.request_calls(), 1);
            assert_eq!(
                b_script.request_calls(),
                0,
                "the lost command was replayed to B"
            );
            let _ = b.release_name(DBUS_SERVICE_NAME).await;
            Ok(())
        })
        .await
    })
}

/// RC-TEST-06 / COMPAT-2: a cancelled reconnect leaves no state change and no orphan work, its
/// late reply changes nothing, and the shared connection, an active subscription and a fresh
/// reconnect all keep working.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rc_a_cancelled_reconnect_leaves_the_connection_and_subscriptions_working()
-> Result<(), Box<dyn Error>> {
    scenario(|a, script, mut client, connection| async move {
        script.reply_with(NEWER);
        let mut events = client.events().await?;
        // Version calls: 1 at construction, 2 when subscribing, 3 for the reconnect.
        script.hold(3, NEWER);
        let mut lookups = Lookups::watch(&connection).await?;
        {
            let mut operation = pin!(client.reconnect(Duration::from_secs(5)));
            poll_until_held(&script, operation.as_mut()).await?;
        }
        // Cancelled while its Version call was held; release the late reply.
        script.go.store(true, Ordering::SeqCst);
        wait_for(&script.sent).await;
        round_trip(&connection).await?;
        assert_eq!(
            client.server_version(),
            INITIAL,
            "a cancelled reconnect changed the version"
        );
        // Nothing keeps running for the cancelled operation.
        let lookups_after_cancel = lookups.settle(Duration::from_millis(50)).await?;
        let calls_after_cancel = script.version_calls();
        Timer::after(Duration::from_millis(300)).await;
        assert_eq!(
            lookups.settle(Duration::from_millis(50)).await?,
            lookups_after_cancel,
            "the cancelled reconnect kept looking up the owner"
        );
        assert_eq!(
            script.version_calls(),
            calls_after_cancel,
            "the cancelled reconnect retried"
        );
        // The active subscription on the same connection still hears the owner.
        let payload = encode_event(&Event::HealthChanged(HealthLevel::Degraded))?;
        a.emit_signal(
            None::<&str>,
            DBUS_OBJECT_PATH,
            DBUS_INTERFACE_NAME,
            "Event",
            &payload,
        )
        .await?;
        match within(WATCHDOG, events.next_event()).await {
            Some(Ok(Event::HealthChanged(HealthLevel::Degraded))) => {}
            other => return Err(format!("the subscription stopped working: {other:?}").into()),
        }
        // A fresh reconnect succeeds and records the version.
        match within(WATCHDOG, client.reconnect(Duration::from_secs(1))).await {
            Some(Ok(())) => {}
            other => return Err(format!("a fresh reconnect failed: {other:?}").into()),
        }
        assert_eq!(client.server_version(), NEWER);
        assert_eq!(
            script.request_calls(),
            0,
            "reconnect sent an ordinary Request"
        );
        Ok(())
    })
}

/// Every method call the client sends, seen through a bus monitor in the broker's routing
/// order: lookups, Version and Request calls counted one by one (RRC-01, TEST-1).
struct Dispatches {
    _monitor: Connection,
    calls: MessageStream,
    lookups: Vec<u32>,
    versions: usize,
    requests: usize,
}

impl Dispatches {
    async fn watch(client: &Connection) -> Result<Self, Box<dyn Error>> {
        let sender = client
            .unique_name()
            .ok_or("a bus connection has a unique name")?
            .to_owned();
        let monitor = Connection::session().await?;
        let rule = zbus::MatchRule::builder()
            .msg_type(Type::MethodCall)
            .sender(sender.as_str())?
            .build();
        zbus::fdo::MonitoringProxy::new(&monitor)
            .await?
            .become_monitor(&[rule], 0)
            .await?;
        let calls = MessageStream::from(&monitor);
        Ok(Self {
            _monitor: monitor,
            calls,
            lookups: Vec::new(),
            versions: 0,
            requests: 0,
        })
    }

    fn record(&mut self, call: &zbus::Message) {
        let header = call.header();
        match header.member().map(|member| member.as_str()) {
            Some("GetNameOwner") => self.lookups.push(header.primary().serial_num().get()),
            Some("Version") => self.versions += 1,
            Some("Request") => self.requests += 1,
            _ => {}
        }
    }

    /// Waits up to `bound` for the next lookup and returns its serial.
    async fn next_lookup(&mut self, bound: Duration) -> Result<Option<u32>, Box<dyn Error>> {
        let before = self.lookups.len();
        let deadline = Instant::now() + bound;
        while self.lookups.len() == before {
            let left = deadline.saturating_duration_since(Instant::now());
            match within(left, self.calls.next()).await {
                Some(Some(call)) => self.record(&call?),
                Some(None) => return Err("the monitor stream ended".into()),
                None => return Ok(None),
            }
        }
        Ok(self.lookups.last().copied())
    }

    /// Every call seen so far, after `quiet` passes with no new one.
    async fn settle(&mut self, quiet: Duration) -> Result<(), Box<dyn Error>> {
        while let Some(Some(call)) = within(quiet, self.calls.next()).await {
            self.record(&call?);
        }
        Ok(())
    }
}

/// Drives `operation` until its owner lookup has been sent and that lookup's reply has been
/// dispatched on the client connection, without letting the operation see the reply. Each poll
/// is followed by an unpolled wait on the monitor, so the operation is never polled while the
/// reply is available; the reply is matched to the lookup by its reply serial. Fails clearly if
/// any phase is not reached, never silently skipping the boundary.
async fn owner_reply_ready_unpolled<T: std::fmt::Debug>(
    dispatches: &mut Dispatches,
    replies: &mut MessageStream,
    mut operation: std::pin::Pin<&mut impl Future<Output = T>>,
) -> Result<(), Box<dyn Error>> {
    let mut lookup = None;
    for _ in 0..50 {
        if let Some(early) = future::poll_once(operation.as_mut()).await {
            return Err(format!("finished before its owner lookup: {early:?}").into());
        }
        lookup = dispatches.next_lookup(Duration::from_millis(20)).await?;
        if lookup.is_some() {
            break;
        }
    }
    let lookup = lookup.ok_or("the reconnect never sent its owner lookup")?;
    let matched = within(WATCHDOG, async {
        while let Some(message) = replies.next().await {
            let Ok(message) = message else { continue };
            let header = message.header();
            if message.message_type() == Type::MethodReturn
                && header.reply_serial().map(|serial| serial.get()) == Some(lookup)
            {
                return true;
            }
        }
        false
    })
    .await;
    if matched != Some(true) {
        return Err("the owner lookup's reply never reached the client".into());
    }
    if dispatches.versions != 0 {
        return Err("Version was sent before the owner reply was handed to reconnect".into());
    }
    Ok(())
}

/// RRC-01 / RCD-BASE-03 / RCD-TEST-01: the review's reproduction. Reconnect's owner lookup is
/// answered while the operation is not being polled; the operation is then resumed after its
/// 50 ms deadline. The one resumed poll must return the typed timeout without sending a new
/// Version (or anything else), and leave the client unchanged and its connection working.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rcd_an_expired_resumption_with_a_ready_owner_reply_sends_no_version()
-> Result<(), Box<dyn Error>> {
    scenario(|_a, script, mut client, connection| async move {
        script.reply_with(NEWER);
        let mut dispatches = Dispatches::watch(&connection).await?;
        let mut replies = MessageStream::from(&connection);
        // Version call 1 was construction; nothing of reconnect's yet.
        assert_eq!(script.version_calls(), 1);
        let outcome = {
            let mut operation = pin!(client.reconnect(BUDGET));
            let first_poll = Instant::now();
            owner_reply_ready_unpolled(&mut dispatches, &mut replies, operation.as_mut()).await?;
            drop(replies);
            let ready_at = first_poll.elapsed();
            if ready_at >= BUDGET {
                return Err(format!("the owner reply took {ready_at:?}, past the budget").into());
            }
            // Unpolled past the deadline with the owner reply ready.
            Timer::at(first_poll + BUDGET * 2).await;
            let versions_before = script.version_calls();
            let resumed_at = first_poll.elapsed();
            let outcome = future::poll_once(operation.as_mut()).await;
            eprintln!(
                "owner reply ready at {ready_at:?}; resumed at {resumed_at:?}; result on that \
                 poll: {}; daemon Version calls before resuming: {versions_before}",
                outcome.is_some()
            );
            outcome
        };
        match outcome {
            Some(Err(ClientError::ReconnectTimeout { timeout, .. })) if timeout == BUDGET => {}
            other => {
                return Err(format!("the resumed poll must time out: {other:?}").into());
            }
        }
        // Everything the client sent has been routed before this round trip's reply.
        round_trip(&connection).await?;
        dispatches.settle(Duration::from_millis(100)).await?;
        let sent = (
            dispatches.lookups.len(),
            dispatches.versions,
            dispatches.requests,
        );
        if sent != (1, 0, 0) {
            return Err(format!(
                "reconnect sent (lookups, Version, Request) = {sent:?} after expiry; expected \
                 (1, 0, 0)"
            )
            .into());
        }
        if script.version_calls() != 1 {
            return Err(format!(
                "the daemon received {} Version calls; expected only construction's",
                script.version_calls()
            )
            .into());
        }
        assert_eq!(client.server_version(), INITIAL);
        // The connection works and a fresh reconnect negotiates normally.
        match within(WATCHDOG, client.reconnect(Duration::from_secs(1))).await {
            Some(Ok(())) => {}
            other => return Err(format!("a fresh reconnect failed: {other:?}").into()),
        }
        assert_eq!(client.server_version(), NEWER);
        Ok(())
    })
}

/// RCD-BASE-03 control / RCD-TEST-02: the same observable owner stage, resumed before the
/// deadline: exactly one reconnect Version, success with the distinct version, no Request.
#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn rcd_a_timely_resumption_with_a_ready_owner_reply_negotiates_once() -> Result<(), Box<dyn Error>>
{
    scenario(|_a, script, mut client, connection| async move {
        script.reply_with(NEWER);
        let mut dispatches = Dispatches::watch(&connection).await?;
        let mut replies = MessageStream::from(&connection);
        let budget = Duration::from_secs(2);
        let outcome = {
            let mut operation = pin!(client.reconnect(budget));
            owner_reply_ready_unpolled(&mut dispatches, &mut replies, operation.as_mut()).await?;
            drop(replies);
            within(WATCHDOG, operation).await
        };
        match outcome {
            Some(Ok(())) => {}
            other => return Err(format!("a timely resumption must succeed: {other:?}").into()),
        }
        assert_eq!(client.server_version(), NEWER);
        round_trip(&connection).await?;
        dispatches.settle(Duration::from_millis(100)).await?;
        let sent = (
            dispatches.lookups.len(),
            dispatches.versions,
            dispatches.requests,
        );
        assert_eq!(
            sent,
            (1, 1, 0),
            "(lookups, Version, Request) sent by reconnect"
        );
        assert_eq!(script.version_calls(), 2);
        assert_eq!(script.request_calls(), 0);
        Ok(())
    })
}

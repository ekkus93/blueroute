//! FU-INTEGRATION-02: the desktop's real `watch_daemon` when its daemon hands the name over to a
//! compatible successor inside the Version check of an event (REV-02). The obsolete event must not
//! reach the UI as a change; the watcher reports the loss, reconnects to the successor and hears it.
//!
//! As in `watch_integration`, the outer test re-runs this binary with `DBUS_SYSTEM_BUS_ADDRESS`
//! set to the private bus of `dbus-run-session`:
//!
//! `dbus-run-session -- cargo test -p blueroute-desktop --test watch_handover --locked -- --ignored`
use std::error::Error;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_desktop::{DaemonSignal, watch_daemon};
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, fdo, interface};

const CHILD: &str = "BLUEROUTE_WATCH_HANDOVER_CHILD";
/// The child scenarios share the daemon's well-known name on one private bus.
static BUS: Mutex<()> = Mutex::new(());
const CURRENT: (u16, u16) = (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor);

/// Hands the name to `successor` inside its `handover`-th Version call. With `returns`, the
/// successor gives it straight back (A→B→A) before A replies (LC-INTEGRATION-01).
struct Daemon {
    handover: Option<usize>,
    returns: bool,
    calls: Arc<AtomicUsize>,
    own: Arc<OnceLock<Connection>>,
    successor: Arc<OnceLock<Connection>>,
}

#[interface(name = "org.blueroute.Service1")]
impl Daemon {
    async fn version(&self) -> (u16, u16) {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.handover == Some(call) {
            let own = self.own.get().expect("own connection");
            own.release_name(DBUS_SERVICE_NAME).await.expect("release");
            let successor = self.successor.get().expect("successor");
            successor
                .request_name(DBUS_SERVICE_NAME)
                .await
                .expect("successor takes the name");
            if self.returns {
                successor
                    .release_name(DBUS_SERVICE_NAME)
                    .await
                    .expect("successor gives the name back");
                own.request_name(DBUS_SERVICE_NAME)
                    .await
                    .expect("A takes it again");
            }
        }
        CURRENT
    }

    fn request(&self, _payload: &str) -> fdo::Result<String> {
        Ok(r#"{"type":"ack"}"#.to_owned())
    }

    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, revision: u64) -> zbus::Result<()>;
}

async fn serve(daemon: Daemon, owns_name: bool) -> Result<Connection, Box<dyn Error>> {
    let own = daemon.own.clone();
    let mut builder = Builder::session()?;
    if owns_name {
        builder = builder.name(DBUS_SERVICE_NAME)?;
    }
    let connection = builder.serve_at(DBUS_OBJECT_PATH, daemon)?.build().await?;
    let _ = own.set(connection.clone());
    Ok(connection)
}

fn daemon(handover: Option<usize>, returns: bool) -> Daemon {
    Daemon {
        handover,
        returns,
        calls: Arc::default(),
        own: Arc::default(),
        successor: Arc::default(),
    }
}

async fn announce(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let interface = connection
        .object_server()
        .interface::<_, Daemon>(DBUS_OBJECT_PATH)
        .await?;
    Daemon::state_changed(interface.signal_emitter(), 1).await?;
    Ok(())
}

async fn until(
    seen: &Mutex<Vec<DaemonSignal>>,
    bound: Duration,
    done: impl Fn(&[DaemonSignal]) -> bool,
) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    while start.elapsed() < bound {
        if done(&seen.lock().unwrap()) {
            return Ok(());
        }
        Timer::after(Duration::from_millis(10)).await;
    }
    Err(format!("timed out; signals so far: {:?}", seen.lock().unwrap()).into())
}

/// Runs one child scenario of this binary against the private bus as the system bus.
fn run_child(scenario: &str) {
    let _bus = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("run under dbus-run-session");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", scenario, "--nocapture"])
        .env(CHILD, "1")
        .env("DBUS_SYSTEM_BUS_ADDRESS", address)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{report}");
    assert!(
        report.contains("1 passed"),
        "the scenario did not run:\n{report}"
    );
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn the_desktop_watcher_follows_an_event_time_handover_to_the_successor() {
    if std::env::var_os(CHILD).is_none() {
        run_child("handover_scenario");
    }
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn the_desktop_watcher_reports_a_returning_daemon_as_a_new_period() {
    if std::env::var_os(CHILD).is_none() {
        run_child("returning_owner_scenario");
    }
}

#[test]
#[ignore = "run by the_desktop_watcher_follows_an_event_time_handover_to_the_successor"]
fn handover_scenario() -> Result<(), Box<dyn Error>> {
    scenario(false)
}

#[test]
#[ignore = "run by the_desktop_watcher_reports_a_returning_daemon_as_a_new_period"]
fn returning_owner_scenario() -> Result<(), Box<dyn Error>> {
    scenario(true)
}

fn scenario(returns: bool) -> Result<(), Box<dyn Error>> {
    if std::env::var_os(CHILD).is_none() {
        return Ok(());
    }
    future::block_on(async {
        use DaemonSignal::{Changed, Connected, Disconnected};
        // Version calls on A: 1 at connect, 2 when subscribing, 3 for A's change signal.
        let a = daemon(Some(3), returns);
        let successor = a.successor.clone();
        let b = serve(daemon(None, false), false).await?;
        let _ = successor.set(b.clone());
        let a = serve(a, true).await?;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        std::thread::spawn(move || {
            future::block_on(watch_daemon(move |signal| {
                sink.lock().unwrap().push(signal)
            }));
        });
        until(&seen, Duration::from_secs(3), |s| s == [Connected]).await?;

        announce(&a).await?;
        // A's obsolete change is not passed on; the loss is, and the current owner (B, or the
        // returning A in a new period) is reached on the backoff.
        until(&seen, Duration::from_secs(6), |s| {
            s == [Connected, Disconnected, Connected]
        })
        .await?;
        announce(if returns { &a } else { &b }).await?;
        until(&seen, Duration::from_secs(1), |s| {
            s == [Connected, Disconnected, Connected, Changed]
        })
        .await?;
        drop((a, b));
        Ok(())
    })
}

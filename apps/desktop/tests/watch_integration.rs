//! CR-INTEGRATION-01/02: the desktop's real `watch_daemon`, unchanged, against a private bus.
//!
//! `watch_daemon` connects to the *system* bus. The outer test re-runs this test binary with
//! `DBUS_SYSTEM_BUS_ADDRESS` pointed at the private session bus from `dbus-run-session` (setting
//! it in-process would need `unsafe`), and the inner test drives a fake daemon there:
//!
//! `dbus-run-session -- cargo test -p blueroute-desktop --test watch_integration --locked -- --ignored`
use std::error::Error;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_desktop::{DaemonSignal, watch_daemon};
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, fdo, interface};

const CHILD: &str = "BLUEROUTE_WATCH_INTEGRATION_CHILD";

struct FakeDaemon {
    version: (u16, u16),
}

#[interface(name = "org.blueroute.Service1")]
impl FakeDaemon {
    fn version(&self) -> (u16, u16) {
        self.version
    }

    fn request(&self, _payload: &str) -> fdo::Result<String> {
        Ok(r#"{"type":"ack"}"#.to_owned())
    }

    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, revision: u64) -> zbus::Result<()>;
}

async fn daemon(version: (u16, u16)) -> Result<Connection, Box<dyn Error>> {
    Ok(Builder::session()?
        .name(DBUS_SERVICE_NAME)?
        .serve_at(DBUS_OBJECT_PATH, FakeDaemon { version })?
        .build()
        .await?)
}

async fn announce(connection: &Connection, revision: u64) -> Result<(), Box<dyn Error>> {
    let interface = connection
        .object_server()
        .interface::<_, FakeDaemon>(DBUS_OBJECT_PATH)
        .await?;
    FakeDaemon::state_changed(interface.signal_emitter(), revision).await?;
    Ok(())
}

async fn stop(connection: Connection) -> Result<(), Box<dyn Error>> {
    connection.release_name(DBUS_SERVICE_NAME).await?;
    connection.close().await?;
    Ok(())
}

async fn until(
    seen: &Mutex<Vec<DaemonSignal>>,
    bound: Duration,
    done: impl Fn(&[DaemonSignal]) -> bool,
) -> Result<Duration, Box<dyn Error>> {
    let start = Instant::now();
    while start.elapsed() < bound {
        if done(&seen.lock().unwrap()) {
            return Ok(start.elapsed());
        }
        Timer::after(Duration::from_millis(10)).await;
    }
    Err(format!("timed out; signals so far: {:?}", seen.lock().unwrap()).into())
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn the_desktop_watcher_follows_idle_loss_change_signals_and_return() {
    if std::env::var_os(CHILD).is_some() {
        return; // the child runs only the scenario below
    }
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("run under dbus-run-session");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "scenario", "--nocapture"])
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

/// Runs only inside the child, where the "system" bus is the private one.
#[test]
#[ignore = "run by the_desktop_watcher_follows_idle_loss_change_signals_and_return"]
fn scenario() -> Result<(), Box<dyn Error>> {
    if std::env::var_os(CHILD).is_none() {
        return Ok(());
    }
    future::block_on(async {
        let current = (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor);
        let first = daemon(current).await?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        // The production watcher, exactly as the Tauri app starts it.
        std::thread::spawn(move || {
            future::block_on(watch_daemon(move |signal| {
                sink.lock().unwrap().push(signal)
            }));
        });
        use DaemonSignal::{Changed, Connected, Disconnected};

        until(&seen, Duration::from_secs(3), |s| s == [Connected]).await?;
        // A daemon change signal makes the UI reread.
        announce(&first, 1).await?;
        until(&seen, Duration::from_secs(1), |s| s == [Connected, Changed]).await?;

        // The daemon stops without any event: reported within 2 s.
        let stopped = Instant::now();
        stop(first).await?;
        until(&seen, Duration::from_secs(2), |s| {
            s.last() == Some(&Disconnected)
        })
        .await?;
        println!(
            "idle loss reached the desktop watcher in {:?}",
            stopped.elapsed()
        );

        // An incompatible daemon never connects.
        let wrong = daemon((current.0 + 1, 0)).await?;
        Timer::after(Duration::from_millis(2500)).await;
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [Connected, Changed, Disconnected]
        );
        stop(wrong).await?;

        // A compatible daemon returns: reconnected on the production backoff schedule.
        let back = daemon(current).await?;
        until(&seen, Duration::from_secs(10), |s| {
            s.last() == Some(&Connected)
        })
        .await?;
        announce(&back, 1).await?;
        until(&seen, Duration::from_secs(1), |s| {
            s.last() == Some(&Changed)
        })
        .await?;
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [Connected, Changed, Disconnected, Connected, Changed]
        );
        Ok(())
    })
}

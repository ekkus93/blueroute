//! Publication of daemon state changes (CR-F1, EVENT-1..EVENT-6).
//!
//! Every change a client could see in its snapshots reaches connected observers as one
//! payload-free `StateChanged(revision)` D-Bus signal (API 1.7). There is one path:
//!
//! - Request dispatch calls [`StatePublisher::committed`] after a mutating request completes, so
//!   a committed change is announced even when nothing in memory differs (trust, durable records).
//! - Workers call [`StatePublisher::observed`] after updating what they own, and every observable
//!   in-memory snapshot is also rechecked every [`STATE_RECHECK`], so a producer that does not
//!   mark (or a future one) still cannot leave observers uninformed. The network list, which reads
//!   the membership store and, during discovery, BlueZ, is rechecked every [`NETWORK_RECHECK`].
//!
//! The emitter runs on its own thread, compares snapshots taken without holding any lock across
//! the emission, and emits off every state lock. Marks only set a flag and wake the thread, so
//! bursts coalesce into one pending invalidation and saturation cannot drop the last one. An
//! emission failure never fails the request that caused it: it is logged without payload and
//! retried on the next recheck until it succeeds. Unchanged observations emit nothing.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use blueroute_protocol::DBUS_OBJECT_PATH;
use futures_lite::future;
use zbus::Connection;

use crate::DaemonService;

/// How often the in-memory snapshots are compared; bounds the latency of an unmarked change.
pub const STATE_RECHECK: Duration = Duration::from_millis(250);
/// How often the network list (membership store and live discovery) is compared.
pub const NETWORK_RECHECK: Duration = Duration::from_secs(1);

/// The marks producers set. Cheap, lock-free for the caller's state, and safe to call anywhere
/// after the producer's own locks are released.
#[derive(Default)]
pub struct StatePublisher {
    /// Publish on the next wake even if no snapshot differs.
    forced: AtomicBool,
    emitter: Mutex<Option<Thread>>,
    /// Marks seen, so unit tests can check which producers mark.
    #[cfg(test)]
    marks: std::sync::atomic::AtomicUsize,
}

impl StatePublisher {
    /// A mutating request finished (successfully or not). Always announced.
    pub fn committed(&self) {
        self.forced.store(true, Ordering::Release);
        self.wake();
    }

    /// A worker updated observable state. Announced only if a snapshot actually changed.
    pub fn observed(&self) {
        self.wake();
    }

    #[cfg(test)]
    pub(crate) fn marks(&self) -> usize {
        self.marks.load(Ordering::SeqCst)
    }

    fn wake(&self) {
        #[cfg(test)]
        self.marks.fetch_add(1, Ordering::SeqCst);
        if let Ok(emitter) = self.emitter.lock()
            && let Some(thread) = emitter.as_ref()
        {
            thread.unpark();
        }
    }
}

/// The daemon-lifetime emitter thread. Dropping it stops the thread.
pub struct StatePublisherWorker {
    stop: Arc<AtomicBool>,
    publisher: Arc<StatePublisher>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for StatePublisherWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut emitter) = self.publisher.emitter.lock() {
            *emitter = None;
        }
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

/// Start publishing `service`'s changes as `StateChanged` signals on `connection`, which must
/// serve `service` at the daemon object path.
pub fn spawn_state_publisher(
    connection: Connection,
    service: DaemonService,
) -> std::io::Result<StatePublisherWorker> {
    let publisher = service.state_publisher();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker_publisher = publisher.clone();
    let thread = thread::Builder::new()
        .name("blueroute-state-publisher".to_owned())
        .spawn(move || run(&connection, &service, &worker_publisher, &worker_stop))?;
    if let Ok(mut emitter) = publisher.emitter.lock() {
        *emitter = Some(thread.thread().clone());
    }
    // A mark set before the thread was registered must not wait for the first recheck.
    thread.thread().unpark();
    Ok(StatePublisherWorker {
        stop,
        publisher,
        thread: Some(thread),
    })
}

fn run(
    connection: &Connection,
    service: &DaemonService,
    publisher: &StatePublisher,
    stop: &AtomicBool,
) {
    let mut published_state = service.observable_state();
    let mut published_networks = future::block_on(service.observable_networks());
    let mut networks_due = Instant::now() + NETWORK_RECHECK;
    let mut revision: u64 = 0;
    let mut failing = false;
    while !stop.load(Ordering::Acquire) {
        thread::park_timeout(STATE_RECHECK);
        if stop.load(Ordering::Acquire) {
            break;
        }
        let forced = publisher.forced.swap(false, Ordering::AcqRel);
        let state = service.observable_state();
        let mut changed = forced || state != published_state;
        let networks = (forced || Instant::now() >= networks_due).then(|| {
            networks_due = Instant::now() + NETWORK_RECHECK;
            future::block_on(service.observable_networks())
        });
        if let Some(networks) = &networks {
            changed |= *networks != published_networks;
        }
        if !changed {
            continue;
        }
        revision = revision.wrapping_add(1);
        match future::block_on(emit_state_changed(connection, revision)) {
            Ok(()) => {
                published_state = state;
                if let Some(networks) = networks {
                    published_networks = networks;
                }
                if failing {
                    failing = false;
                    log_emission("recovered");
                }
            }
            Err(_) => {
                // Keep the invalidation pending; the next recheck retries it.
                publisher.forced.store(true, Ordering::Release);
                if !failing {
                    failing = true;
                    log_emission("failed");
                }
            }
        }
    }
}

async fn emit_state_changed(connection: &Connection, revision: u64) -> zbus::Result<()> {
    let interface = connection
        .object_server()
        .interface::<_, DaemonService>(DBUS_OBJECT_PATH)
        .await?;
    DaemonService::state_changed(interface.signal_emitter(), revision).await
}

/// Payload-free: no state, no error text.
fn log_emission(outcome: &str) {
    eprintln!(
        "{}",
        serde_json::json!({
            "component": "daemon_runtime",
            "event": "state_changed_signal",
            "outcome": outcome,
        })
    );
}

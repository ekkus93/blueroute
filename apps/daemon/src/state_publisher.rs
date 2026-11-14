//! Publication of daemon state changes (CR-F1, EVENT-1..EVENT-6; FU-01, PUB-1..PUB-4).
//!
//! Every change a client could see in its snapshots reaches connected observers as one
//! payload-free `StateChanged(revision)` D-Bus signal (API 1.7). There is one path:
//!
//! - Request dispatch calls [`StatePublisher::committed`] after a mutating request completes, so
//!   a committed change is announced even when nothing in memory differs (trust, durable records).
//! - Workers call [`StatePublisher::observed`] after updating what they own, and every observable
//!   in-memory snapshot is also rechecked every [`STATE_RECHECK`], so a producer that does not
//!   mark (or a future one) still cannot leave observers uninformed.
//!
//! Two threads keep slow observation out of delivery (FU-01):
//!
//! - The **sampler** owns the only slow read, the network list (membership store and, during
//!   discovery, BlueZ). It takes one sample at a time, every [`NETWORK_RECHECK`] or sooner after a
//!   committed request, bounds each by [`SAMPLE_DEADLINE`] (a timeout counts as a failed read,
//!   never as an empty list), stores the result, and wakes the emitter. Samples are taken one
//!   after another, so a result can never overwrite a newer one.
//! - The **emitter** never waits for a sample. It compares the cheap in-memory snapshot and the
//!   latest stored sample with what it last announced, and emits off every state lock.
//!
//! Startup handoff (FU-05): a client can read a snapshot before the emitter's baseline exists, so
//! a change before it would be absorbed silently. The emitter therefore announces once as soon as
//! it runs and once when the first network sample lands, whether or not anything differs. A
//! client that subscribes and then reads (as the shared watcher does) can never be left on a
//! snapshot from before the baseline.
//!
//! Marks only set a flag and wake a thread, so bursts coalesce into one pending invalidation and
//! cannot drop the last one. A failed emission is logged without payload, kept pending and retried
//! on the next recheck; it never fails the request that caused it. Unchanged observations emit
//! nothing. Shutdown sets a stop flag that cancels a pending sample or emission within
//! [`STOP_POLL`], then joins both threads: nothing is detached.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use async_io::Timer;
use blueroute_protocol::DBUS_OBJECT_PATH;
use futures_lite::future;
use zbus::Connection;

use crate::DaemonService;

/// How often the in-memory snapshots are compared; bounds the latency of an unmarked change.
pub const STATE_RECHECK: Duration = Duration::from_millis(250);
/// How often the network list (membership store and live discovery) is sampled.
pub const NETWORK_RECHECK: Duration = Duration::from_secs(1);
/// The longest one network sample may take; a slower read counts as failed.
pub const SAMPLE_DEADLINE: Duration = Duration::from_secs(10);
/// How quickly a stop request cancels a pending sample or emission.
pub const STOP_POLL: Duration = Duration::from_millis(50);

/// The marks producers set. Cheap, lock-free for the caller's state, and safe to call anywhere
/// after the producer's own locks are released.
#[derive(Default)]
pub struct StatePublisher {
    /// Publish on the next wake even if no snapshot differs.
    forced: AtomicBool,
    emitter: Mutex<Option<Thread>>,
    sampler: Mutex<Option<Thread>>,
    /// Marks seen, so unit tests can check which producers mark.
    #[cfg(test)]
    marks: std::sync::atomic::AtomicUsize,
}

impl StatePublisher {
    /// A mutating request finished (successfully or not). Always announced, and the network
    /// list is sampled again soon, since a request may have changed it.
    pub fn committed(&self) {
        self.forced.store(true, Ordering::Release);
        self.wake();
        unpark(&self.sampler);
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
        unpark(&self.emitter);
    }
}

fn unpark(slot: &Mutex<Option<Thread>>) {
    if let Ok(thread) = slot.lock()
        && let Some(thread) = thread.as_ref()
    {
        thread.unpark();
    }
}

/// The latest network sample: `None` until the first one, then the encoded list, or `None`
/// inside when the read failed or timed out.
type Samples = Mutex<Option<Option<String>>>;

/// The daemon-lifetime emitter and sampler threads. Dropping it stops and joins both, within
/// about [`STOP_POLL`] even while a sample is pending.
pub struct StatePublisherWorker {
    stop: Arc<AtomicBool>,
    publisher: Arc<StatePublisher>,
    threads: Vec<JoinHandle<()>>,
}

impl Drop for StatePublisherWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for slot in [&self.publisher.emitter, &self.publisher.sampler] {
            if let Ok(mut thread) = slot.lock() {
                *thread = None;
            }
        }
        for thread in &self.threads {
            thread.thread().unpark();
        }
        for thread in self.threads.drain(..) {
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
    let samples: Arc<Samples> = Arc::default();
    let mut worker = StatePublisherWorker {
        stop: stop.clone(),
        publisher: publisher.clone(),
        threads: Vec::new(),
    };

    let emitter = {
        let (stop, publisher, samples, service) = (
            stop.clone(),
            publisher.clone(),
            samples.clone(),
            service.clone(),
        );
        thread::Builder::new()
            .name("blueroute-state-publisher".to_owned())
            .spawn(move || emit_loop(&connection, &service, &publisher, &samples, &stop))?
    };
    if let Ok(mut slot) = publisher.emitter.lock() {
        *slot = Some(emitter.thread().clone());
    }
    let emitter_thread = emitter.thread().clone();
    worker.threads.push(emitter);

    let sampler = {
        let (stop, samples) = (stop.clone(), samples.clone());
        thread::Builder::new()
            .name("blueroute-network-sampler".to_owned())
            .spawn(move || sample_loop(&service, &samples, &emitter_thread, &stop))?
    };
    if let Ok(mut slot) = publisher.sampler.lock() {
        *slot = Some(sampler.thread().clone());
    }
    worker.threads.push(sampler);

    // Startup handoff (FU-05): a client may have read a snapshot before the baselines exist, and a
    // change before them would otherwise be absorbed silently. So announce once as soon as the
    // emitter runs, and again when the first network sample lands (see `emit_loop`).
    publisher.forced.store(true, Ordering::Release);
    // A mark set before the threads were registered must not wait for the first recheck.
    for thread in &worker.threads {
        thread.thread().unpark();
    }
    Ok(worker)
}

/// Resolves once `stop` is set, checking every [`STOP_POLL`].
async fn stopped(stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        Timer::after(STOP_POLL).await;
    }
}

fn sample_loop(service: &DaemonService, samples: &Samples, emitter: &Thread, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        let bounded = async {
            future::or(service.observable_networks(), async {
                Timer::after(SAMPLE_DEADLINE).await;
                log_sample_timeout();
                None
            })
            .await
        };
        let sample = future::block_on(future::or(async { Some(bounded.await) }, async {
            stopped(stop).await;
            None
        }));
        let Some(sample) = sample else {
            break; // stopped while sampling
        };
        if let Ok(mut slot) = samples.lock() {
            *slot = Some(sample);
        }
        emitter.unpark();
        thread::park_timeout(NETWORK_RECHECK);
    }
}

fn emit_loop(
    connection: &Connection,
    service: &DaemonService,
    publisher: &StatePublisher,
    samples: &Samples,
    stop: &AtomicBool,
) {
    let mut published_state = service.observable_state();
    // `None` until the first sample, which becomes the baseline without an announcement.
    let mut published_networks: Option<Option<String>> = None;
    let mut revision: u64 = 0;
    let mut failing = false;
    while !stop.load(Ordering::Acquire) {
        thread::park_timeout(STATE_RECHECK);
        if stop.load(Ordering::Acquire) {
            break;
        }
        let forced = publisher.forced.swap(false, Ordering::AcqRel);
        let state = service.observable_state();
        let networks = samples.lock().ok().and_then(|slot| slot.clone());
        // The first sample becomes the baseline and is announced once, since a client may have
        // read the network list before it existed (FU-05).
        let first_sample = published_networks.is_none() && networks.is_some();
        if first_sample {
            published_networks = networks.clone();
        }
        let networks_changed = networks.is_some() && networks != published_networks;
        if !(forced || first_sample || networks_changed || state != published_state) {
            continue;
        }
        revision = revision.wrapping_add(1);
        let emitted = future::block_on(future::or(
            async { Some(emit_state_changed(connection, revision).await) },
            async {
                stopped(stop).await;
                None
            },
        ));
        match emitted {
            None => break,
            Some(Ok(())) => {
                published_state = state;
                if networks.is_some() {
                    published_networks = networks;
                }
                if failing {
                    failing = false;
                    log_emission("recovered");
                }
            }
            Some(Err(_)) => {
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

/// Payload-free: a network sample took longer than [`SAMPLE_DEADLINE`].
fn log_sample_timeout() {
    eprintln!(
        "{}",
        serde_json::json!({
            "component": "daemon_runtime",
            "event": "network_sample",
            "outcome": "timed_out",
        })
    );
}

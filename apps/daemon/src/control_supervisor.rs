//! Owner of the long-lived authenticated control session for a joined network (P6-004, D4).
//!
//! `JoinControlSession` only names a network, so something must own the live TLS session, run its
//! once-per-second ownership checks and heartbeats, and stop it on request. That owner is one
//! worker thread per network, started by `start` and ended by `stop`. The session borrows trust
//! state, so it is created and dropped entirely inside the worker; nothing self-referential
//! crosses threads.
//!
//! Dropping a future is not cleanup: a cancelled `start` leaves the worker registered, so a later
//! `stop` (or reconciliation) can still find and end it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::thread::JoinHandle;

use blueroute_core::{CoreError, ErrorKind, NetworkId};

#[derive(Debug)]
struct Handshake {
    result: Option<Result<(), CoreError>>,
    waker: Option<Waker>,
}

/// Lets the worker report whether the session was established, exactly once.
pub struct Established(Arc<Mutex<Handshake>>);

impl Established {
    pub fn report(&self, result: Result<(), CoreError>) {
        let waker = {
            let Ok(mut state) = self.0.lock() else { return };
            if state.result.is_none() {
                state.result = Some(result);
            }
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// What a supervised session is doing, for health reporting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SupervisedState {
    Establishing,
    Running,
    /// The worker ended; `None` means it stopped because it was asked to.
    Ended(Option<String>),
}

struct Supervised {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    state: Arc<Mutex<SupervisedState>>,
}

#[derive(Default)]
pub struct ControlSupervisors {
    sessions: Mutex<HashMap<NetworkId, Supervised>>,
}

fn lock_error() -> CoreError {
    CoreError::new(
        ErrorKind::Internal,
        "control supervisor state lock poisoned; refusing to continue",
    )
}

impl ControlSupervisors {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start the worker for `network` and wait until it reports the session established.
    ///
    /// `run` executes on the worker thread: it must establish the session, call
    /// `established.report(..)`, then supervise until `stop` becomes true. Returning without
    /// reporting counts as an establishment failure. On failure the worker is joined and removed.
    pub async fn start<Run>(&self, network: NetworkId, run: Run) -> Result<(), CoreError>
    where
        Run: FnOnce(&AtomicBool, &Established) -> Result<(), CoreError> + Send + 'static,
    {
        let handshake = Arc::new(Mutex::new(Handshake {
            result: None,
            waker: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(SupervisedState::Establishing));
        {
            let mut sessions = self.sessions.lock().map_err(|_| lock_error())?;
            if sessions.contains_key(&network) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "a control session is already supervised for this network",
                ));
            }
            let worker_stop = stop.clone();
            let worker_state = state.clone();
            let established = Established(handshake.clone());
            let thread = std::thread::Builder::new()
                .name("blueroute-control-supervisor".into())
                .spawn(move || {
                    // A panic must become an error, otherwise `start` would wait forever.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(&worker_stop, &established)
                    }))
                    .unwrap_or_else(|_| {
                        Err(CoreError::new(
                            ErrorKind::Internal,
                            "control session worker panicked",
                        ))
                    });
                    // Without an explicit report, the outcome is also the establishment result.
                    established.report(match &outcome {
                        Ok(()) => Err(CoreError::new(
                            ErrorKind::PanFailure,
                            "control session worker ended before reporting establishment",
                        )),
                        Err(error) => Err(error.clone()),
                    });
                    if let Ok(mut state) = worker_state.lock() {
                        *state = SupervisedState::Ended(outcome.err().map(|e| e.to_string()));
                    }
                })
                .map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::Internal,
                        "failed to start the control session worker",
                        error.to_string(),
                    )
                })?;
            sessions.insert(
                network,
                Supervised {
                    stop,
                    thread,
                    state: state.clone(),
                },
            );
        }

        let result = std::future::poll_fn(|cx| {
            let Ok(mut shared) = handshake.lock() else {
                return Poll::Ready(Err(lock_error()));
            };
            match shared.result.take() {
                Some(result) => Poll::Ready(result),
                None => {
                    shared.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await;
        match result {
            Ok(()) => {
                if let Ok(mut state) = state.lock()
                    && *state == SupervisedState::Establishing
                {
                    *state = SupervisedState::Running;
                }
                Ok(())
            }
            Err(error) => {
                // The worker already stopped on its own; collect it so nothing is left behind.
                let _ = self.stop(network);
                Err(error)
            }
        }
    }

    /// Ask the worker to stop and wait for it. Idempotent: an unknown network is a no-op.
    ///
    /// The wait is bounded by the worker's own deadlines (one supervision tick), and blocks the
    /// caller's thread for that long.
    pub fn stop(&self, network: NetworkId) -> Result<(), CoreError> {
        let supervised = self
            .sessions
            .lock()
            .map_err(|_| lock_error())?
            .remove(&network);
        let Some(supervised) = supervised else {
            return Ok(());
        };
        supervised.stop.store(true, Ordering::Release);
        supervised.thread.join().map_err(|_| {
            CoreError::new(
                ErrorKind::Internal,
                "control session worker panicked while stopping",
            )
        })
    }

    pub fn state(&self, network: NetworkId) -> Option<SupervisedState> {
        let sessions = self.sessions.lock().ok()?;
        let state = sessions.get(&network)?.state.lock().ok()?;
        Some(state.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use super::*;

    fn net(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not reached in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn supervise_until_stopped(stop: &AtomicBool, ticks: &AtomicUsize) -> Result<(), CoreError> {
        while !stop.load(Ordering::Acquire) {
            ticks.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }

    #[test]
    fn established_session_runs_until_stop_and_is_then_joined() {
        let supervisors = ControlSupervisors::new();
        let ticks = Arc::new(AtomicUsize::new(0));
        let worker_ticks = ticks.clone();
        futures_lite::future::block_on(supervisors.start(net(1), move |stop, established| {
            established.report(Ok(()));
            supervise_until_stopped(stop, &worker_ticks)
        }))
        .unwrap();
        assert_eq!(supervisors.state(net(1)), Some(SupervisedState::Running));
        until(|| ticks.load(Ordering::SeqCst) > 0);
        supervisors.stop(net(1)).unwrap();
        // Joined and removed; stopping again is a harmless no-op.
        assert_eq!(supervisors.state(net(1)), None);
        let after = ticks.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(ticks.load(Ordering::SeqCst), after);
        supervisors.stop(net(1)).unwrap();
    }

    #[test]
    fn establishment_failure_is_returned_and_leaves_no_worker() {
        let supervisors = ControlSupervisors::new();
        let error = futures_lite::future::block_on(supervisors.start(net(2), |_, established| {
            let failure = CoreError::new(ErrorKind::AuthenticationFailed, "pin mismatch");
            established.report(Err(failure.clone()));
            Err(failure)
        }))
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
        assert_eq!(supervisors.state(net(2)), None);
        // The network can be started again afterwards.
        futures_lite::future::block_on(supervisors.start(net(2), |stop, established| {
            established.report(Ok(()));
            supervise_until_stopped(stop, &AtomicUsize::new(0))
        }))
        .unwrap();
        supervisors.stop(net(2)).unwrap();
    }

    #[test]
    fn returning_without_reporting_counts_as_an_establishment_failure() {
        let supervisors = ControlSupervisors::new();
        let error =
            futures_lite::future::block_on(supervisors.start(net(3), |_, _| Ok(()))).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PanFailure);
        assert_eq!(supervisors.state(net(3)), None);
    }

    #[test]
    fn a_second_session_for_the_same_network_is_refused() {
        let supervisors = ControlSupervisors::new();
        futures_lite::future::block_on(supervisors.start(net(4), |stop, established| {
            established.report(Ok(()));
            supervise_until_stopped(stop, &AtomicUsize::new(0))
        }))
        .unwrap();
        let error = futures_lite::future::block_on(supervisors.start(net(4), |_, established| {
            established.report(Ok(()));
            Ok(())
        }))
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        // The original session is untouched and still running.
        assert_eq!(supervisors.state(net(4)), Some(SupervisedState::Running));
        supervisors.stop(net(4)).unwrap();
    }

    #[test]
    fn a_session_that_ends_by_itself_is_reported_not_hidden() {
        let supervisors = ControlSupervisors::new();
        futures_lite::future::block_on(supervisors.start(net(5), |_, established| {
            established.report(Ok(()));
            Err(CoreError::new(ErrorKind::PanFailure, "PANU link lost"))
        }))
        .unwrap();
        until(|| matches!(supervisors.state(net(5)), Some(SupervisedState::Ended(_))));
        let Some(SupervisedState::Ended(Some(reason))) = supervisors.state(net(5)) else {
            panic!("expected an ended session with a reason");
        };
        assert!(reason.contains("PANU link lost"));
        supervisors.stop(net(5)).unwrap();
        assert_eq!(supervisors.state(net(5)), None);
    }

    #[test]
    fn a_panicking_worker_is_an_error_not_a_hang() {
        let supervisors = ControlSupervisors::new();
        let error = futures_lite::future::block_on(supervisors.start(net(6), |_, _| {
            panic!("worker bug");
        }))
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Internal);
        assert_eq!(supervisors.state(net(6)), None);
    }
}

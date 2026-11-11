//! Bounded reconnection for a restarted joined client (P6-009 S5, maintainer answer 3).
//!
//! Runs [`reconcile_client_once`] with capped exponential backoff and **stops after a bounded
//! number of attempts**, then waits for the user: durable membership is never touched, nothing
//! is retried forever, and a `Degraded` outcome (something that needs a person) ends the run at
//! once. The caller reports health from the progress callback and starts a fresh run when the
//! user asks to retry.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use blueroute_core::{CoreError, IpPrefix, NetworkId};
use blueroute_linux::PeerHandle;

use crate::{ClientHost, ClientReconcileOutcome, ClientRuntime, reconcile_client_once};

/// How patiently to reconnect. `max_attempts` of 0 is treated as 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl ReconnectPolicy {
    /// The wait after `failed_attempts` consecutive failures: doubles each time, never above
    /// `max_delay`, and saturates instead of overflowing.
    pub fn delay_after(&self, failed_attempts: u32) -> Duration {
        let doublings = failed_attempts.saturating_sub(1).min(32);
        let factor = 1u32.checked_shl(doublings).unwrap_or(u32::MAX);
        self.initial_delay
            .checked_mul(factor)
            .unwrap_or(self.max_delay)
            .min(self.max_delay)
    }
}

/// What a run is doing, for health and events.
#[derive(Debug, Eq, PartialEq)]
pub enum ReconnectProgress<'a> {
    Attempting {
        attempt: u32,
    },
    Waiting {
        attempt: u32,
        delay: Duration,
        error: &'a CoreError,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum ReconnectEnd {
    /// The supervisor started; health turns healthy when it reports `established`.
    Started { attempts: u32 },
    /// Needs a person. No further attempts were made.
    Degraded(CoreError),
    /// The attempt budget is spent. Membership is intact; the user must ask to retry.
    GaveUp { attempts: u32, last: CoreError },
    /// Stopped by the caller (leave or shutdown) before an attempt.
    Stopped,
}

/// Longest single sleep, so a stop request is noticed promptly.
const SLEEP_SLICE: Duration = Duration::from_millis(250);

#[allow(clippy::too_many_arguments)]
pub async fn run_reconnect<Sleep, SleepFuture>(
    runtime: &dyn ClientRuntime,
    network: NetworkId,
    host: ClientHost,
    hint: Option<PeerHandle>,
    expected_lease: IpPrefix,
    policy: ReconnectPolicy,
    stop: &AtomicBool,
    sleep: Sleep,
    progress: &dyn Fn(&ReconnectProgress<'_>),
) -> ReconnectEnd
where
    Sleep: Fn(Duration) -> SleepFuture,
    SleepFuture: Future<Output = ()>,
{
    let budget = policy.max_attempts.max(1);
    let mut attempt = 0;
    loop {
        if stop.load(Ordering::SeqCst) {
            return ReconnectEnd::Stopped;
        }
        attempt += 1;
        progress(&ReconnectProgress::Attempting { attempt });
        let error =
            match reconcile_client_once(runtime, network, host, hint.clone(), expected_lease).await
            {
                ClientReconcileOutcome::Started => {
                    return ReconnectEnd::Started { attempts: attempt };
                }
                ClientReconcileOutcome::Degraded(error) => return ReconnectEnd::Degraded(error),
                ClientReconcileOutcome::Reconnecting(error) => error,
            };
        if attempt >= budget {
            return ReconnectEnd::GaveUp {
                attempts: attempt,
                last: error,
            };
        }
        let delay = policy.delay_after(attempt);
        progress(&ReconnectProgress::Waiting {
            attempt,
            delay,
            error: &error,
        });
        let mut remaining = delay;
        while !remaining.is_zero() {
            if stop.load(Ordering::SeqCst) {
                return ReconnectEnd::Stopped;
            }
            let slice = remaining.min(SLEEP_SLICE);
            sleep(slice).await;
            remaining -= slice;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex;

    use blueroute_core::{ErrorKind, NodeId};
    use blueroute_linux::{
        BackendFuture, InterfaceAddress, NetworkInterfaceHandle, PanAttachment, PanRole,
    };
    use blueroute_protocol::peer_pin::SpkiSha256;

    use super::*;
    use crate::{JoinIpLease, JoinPanLink, ObservedClient};

    fn net() -> NetworkId {
        NetworkId::from_bytes([0x61; 16])
    }

    fn peer() -> PeerHandle {
        PeerHandle::new("/org/bluez/hci0/dev_AA_00_00_00_00_01").unwrap()
    }

    fn lease_prefix() -> IpPrefix {
        IpPrefix::new("192.168.77.2".parse().unwrap(), 24).unwrap()
    }

    fn host() -> ClientHost {
        ClientHost {
            node: NodeId::from_bytes([0x62; 16]),
            fingerprint: SpkiSha256::from_verified_certificate_digest([0x63; 32]).unwrap(),
        }
    }

    /// Each `observe` pops the next scripted result; a link that is up with its lease present
    /// leads straight to the supervisor, so the script alone drives the outcome.
    struct Scripted {
        observations: Mutex<Vec<Result<(), CoreError>>>,
        observed: Mutex<u32>,
    }

    impl Scripted {
        fn new(script: Vec<Result<(), CoreError>>) -> Self {
            Self {
                observations: Mutex::new(script),
                observed: Mutex::new(0),
            }
        }

        fn observed(&self) -> u32 {
            *self.observed.lock().unwrap()
        }
    }

    impl ClientRuntime for Scripted {
        fn observe(
            &self,
            network: NetworkId,
            _hint: Option<PeerHandle>,
        ) -> BackendFuture<'_, ObservedClient> {
            Box::pin(async move {
                *self.observed.lock().unwrap() += 1;
                let next = self.observations.lock().unwrap().remove(0);
                next.map(|()| ObservedClient {
                    panu: Some(PanAttachment {
                        role: PanRole::Panu,
                        interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
                        peer: Some(peer()),
                    }),
                    owned_addresses: vec![InterfaceAddress {
                        interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
                        prefix: lease_prefix(),
                        owner: network,
                    }],
                    owned_profile: true,
                })
            })
        }

        fn lease_segment_free(&self, _network: NetworkId) -> BackendFuture<'_, bool> {
            Box::pin(async { Ok(true) })
        }

        fn reestablish_panu(
            &self,
            _network: NetworkId,
            _hint: Option<PeerHandle>,
        ) -> BackendFuture<'_, JoinPanLink> {
            Box::pin(async { Err(CoreError::new(ErrorKind::Internal, "not expected")) })
        }

        fn apply_lease(
            &self,
            _network: NetworkId,
            _link: &JoinPanLink,
        ) -> BackendFuture<'_, JoinIpLease> {
            Box::pin(async { Err(CoreError::new(ErrorKind::Internal, "not expected")) })
        }

        fn start_supervisor(
            &self,
            _network: NetworkId,
            _host: ClientHost,
            _link: JoinPanLink,
            _lease: JoinIpLease,
        ) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn transient() -> Result<(), CoreError> {
        Err(CoreError::new(ErrorKind::PanFailure, "host not in range"))
    }

    fn seconds(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    struct Harness {
        slept: Cell<Duration>,
        events: RefCell<Vec<String>>,
        stop: AtomicBool,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                slept: Cell::new(Duration::ZERO),
                events: RefCell::new(Vec::new()),
                stop: AtomicBool::new(false),
            }
        }

        fn run(&self, runtime: &Scripted, policy: ReconnectPolicy) -> ReconnectEnd {
            futures_lite::future::block_on(run_reconnect(
                runtime,
                net(),
                host(),
                Some(peer()),
                lease_prefix(),
                policy,
                &self.stop,
                |slice| {
                    self.slept.set(self.slept.get() + slice);
                    async {}
                },
                &|progress| {
                    self.events.borrow_mut().push(match progress {
                        ReconnectProgress::Attempting { attempt } => format!("attempt {attempt}"),
                        ReconnectProgress::Waiting { attempt, delay, .. } => {
                            format!("wait {attempt} {}s", delay.as_secs())
                        }
                    });
                },
            ))
        }
    }

    fn policy(max_attempts: u32) -> ReconnectPolicy {
        ReconnectPolicy {
            max_attempts,
            initial_delay: seconds(2),
            max_delay: seconds(60),
        }
    }

    #[test]
    fn the_delay_doubles_caps_and_never_overflows() {
        let policy = policy(8);
        let delays: Vec<u64> = (1..=8).map(|n| policy.delay_after(n).as_secs()).collect();
        assert_eq!(delays, [2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(policy.delay_after(0), seconds(2));
        assert_eq!(policy.delay_after(u32::MAX), seconds(60));
    }

    #[test]
    fn success_on_a_later_attempt_waits_between_failures_only() {
        let runtime = Scripted::new(vec![transient(), transient(), Ok(())]);
        let harness = Harness::new();
        assert_eq!(
            harness.run(&runtime, policy(5)),
            ReconnectEnd::Started { attempts: 3 }
        );
        assert_eq!(harness.slept.get(), seconds(2 + 4));
        assert_eq!(
            *harness.events.borrow(),
            [
                "attempt 1",
                "wait 1 2s",
                "attempt 2",
                "wait 2 4s",
                "attempt 3"
            ]
        );
    }

    #[test]
    fn the_attempt_budget_is_spent_then_it_stops_and_waits_for_the_user() {
        let runtime = Scripted::new(vec![transient(), transient(), transient(), Ok(())]);
        let harness = Harness::new();
        let end = harness.run(&runtime, policy(3));
        match end {
            ReconnectEnd::GaveUp { attempts, last } => {
                assert_eq!(attempts, 3);
                assert_eq!(last.kind(), ErrorKind::PanFailure);
            }
            other => panic!("expected GaveUp, got {other:?}"),
        }
        // Exactly three attempts: the scripted success was never reached on its own, and there
        // is no wait after the final failure.
        assert_eq!(runtime.observed(), 3);
        assert_eq!(harness.slept.get(), seconds(2 + 4));
    }

    #[test]
    fn something_that_needs_a_person_ends_the_run_at_once() {
        let runtime = Scripted::new(vec![
            transient(),
            Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "bad host key",
            )),
            Ok(()),
        ]);
        let harness = Harness::new();
        match harness.run(&runtime, policy(5)) {
            ReconnectEnd::Degraded(error) => {
                assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
            }
            other => panic!("expected Degraded, got {other:?}"),
        }
        assert_eq!(runtime.observed(), 2);
    }

    #[test]
    fn a_stop_before_the_first_attempt_does_nothing() {
        let runtime = Scripted::new(vec![Ok(())]);
        let harness = Harness::new();
        harness.stop.store(true, Ordering::SeqCst);
        assert_eq!(harness.run(&runtime, policy(5)), ReconnectEnd::Stopped);
        assert_eq!(runtime.observed(), 0);
    }

    #[test]
    fn a_stop_during_a_wait_ends_the_run_without_another_attempt() {
        let runtime = Scripted::new(vec![transient(), Ok(())]);
        let harness = Harness::new();
        let end = futures_lite::future::block_on(run_reconnect(
            &runtime,
            net(),
            host(),
            Some(peer()),
            lease_prefix(),
            policy(5),
            &harness.stop,
            |slice| {
                harness.slept.set(harness.slept.get() + slice);
                harness.stop.store(true, Ordering::SeqCst);
                async {}
            },
            &|_| {},
        ));
        assert_eq!(end, ReconnectEnd::Stopped);
        assert_eq!(runtime.observed(), 1);
        assert!(harness.slept.get() <= Duration::from_millis(250));
    }

    #[test]
    fn a_zero_budget_still_makes_one_attempt() {
        let runtime = Scripted::new(vec![transient()]);
        let harness = Harness::new();
        assert!(matches!(
            harness.run(&runtime, policy(0)),
            ReconnectEnd::GaveUp { attempts: 1, .. }
        ));
        assert_eq!(harness.slept.get(), Duration::ZERO);
    }
}

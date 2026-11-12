//! Restart reconnection of a joined client as a daemon service (P6-009 S5b).
//!
//! At startup, a persisted `Client` is reconnected in the **background** so a slow radio can
//! never block daemon start or D-Bus. The run is bounded (see `ReconnectPolicy`): it ends in
//! `Established`, `NeedsAttention` or `GaveUp`, never retries forever, and never touches durable
//! membership. The user can start a fresh run with `retry`. Leave cancels the run first.
//!
//! Health is derived from the published state, so the UI can show `Reconnecting` while trying
//! and `Degraded` when it needs the user, without a separate channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use blueroute_core::{
    CoreError, ErrorKind, HealthLevel, Ipv4AddressPool, Ipv4StarAddressPlan, MembershipRole,
    MembershipState, NetworkId,
};
use blueroute_linux::{BackendFuture, NetworkMembershipStore, PeerHandle};
use blueroute_protocol::ClientLinkState;

use crate::{
    ClientHost, ClientRecordStore, ClientRuntime, ControlSupervisors, JoinIpLease, JoinPanLink,
    ObservedClient, PeerPinStore, ReconnectEnd, ReconnectPolicy, ReconnectProgress,
    current_network, run_reconnect, single_approved_host,
};

/// The health a published client-link state implies. `None` (not a client) adds nothing.
pub fn health_for_client_link(state: Option<&ClientLinkState>) -> HealthLevel {
    match state {
        None | Some(ClientLinkState::Established) => HealthLevel::Healthy,
        Some(ClientLinkState::Connecting { .. } | ClientLinkState::Waiting { .. }) => {
            HealthLevel::Reconnecting
        }
        Some(ClientLinkState::NeedsAttention { .. } | ClientLinkState::GaveUp { .. }) => {
            HealthLevel::Degraded
        }
    }
}

type Observer = Arc<dyn Fn(Option<&ClientLinkState>) + Send + Sync>;
type Sleeper = Arc<dyn Fn(Duration) + Send + Sync>;

struct Run {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Inner {
    state: Option<ClientLinkState>,
    /// Bumped whenever a run starts or is cancelled; a stale run's updates are ignored.
    generation: u64,
    run: Option<Run>,
}

pub struct ClientReconnector {
    memberships: NetworkMembershipStore,
    pins: PeerPinStore,
    records: Arc<ClientRecordStore>,
    runtime: Arc<dyn ClientRuntime>,
    supervisors: Arc<ControlSupervisors>,
    pool: Ipv4AddressPool,
    policy: ReconnectPolicy,
    sleeper: Sleeper,
    observer: Mutex<Option<Observer>>,
    inner: Mutex<Inner>,
}

fn lock_error() -> CoreError {
    CoreError::new(
        ErrorKind::Internal,
        "client reconnection state lock poisoned; refusing to use unverified state",
    )
}

impl ClientReconnector {
    pub fn new(
        memberships: NetworkMembershipStore,
        pins: PeerPinStore,
        records: Arc<ClientRecordStore>,
        runtime: Arc<dyn ClientRuntime>,
        supervisors: Arc<ControlSupervisors>,
        pool: Ipv4AddressPool,
        policy: ReconnectPolicy,
    ) -> Arc<Self> {
        Self::with_sleeper(
            memberships,
            pins,
            records,
            runtime,
            supervisors,
            pool,
            policy,
            Arc::new(thread::sleep),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_sleeper(
        memberships: NetworkMembershipStore,
        pins: PeerPinStore,
        records: Arc<ClientRecordStore>,
        runtime: Arc<dyn ClientRuntime>,
        supervisors: Arc<ControlSupervisors>,
        pool: Ipv4AddressPool,
        policy: ReconnectPolicy,
        sleeper: Sleeper,
    ) -> Arc<Self> {
        Arc::new(Self {
            memberships,
            pins,
            records,
            runtime,
            supervisors,
            pool,
            policy,
            sleeper,
            observer: Mutex::new(None),
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Called with every published state change (for health reporting).
    pub fn set_observer(&self, observer: Observer) {
        if let Ok(mut slot) = self.observer.lock() {
            *slot = Some(observer);
        }
    }

    pub fn state(&self) -> Option<ClientLinkState> {
        self.inner.lock().ok().and_then(|inner| inner.state.clone())
    }

    /// Startup: reconnect in the background if durable membership says this node is a client.
    /// `Ok(false)` means it is not (a host, or in no network); nothing was started.
    pub fn start(self: &Arc<Self>) -> Result<bool, CoreError> {
        match self.begin() {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::InvalidState && !self.is_client()? => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// The user asked to try again. Refused unless this is a joined client with no run active.
    pub fn retry(self: &Arc<Self>) -> Result<(), CoreError> {
        self.begin()
    }

    /// Stop the run for a leave. A late result from it is ignored, and `start_supervisor` is
    /// refused from now on, so it cannot bring a session back after the leave tears it down.
    pub fn cancel(&self) {
        let notify = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            let had_run = inner.run.is_some() || inner.state.is_some();
            inner.generation += 1;
            if let Some(run) = &inner.run {
                run.stop.store(true, Ordering::SeqCst);
            }
            inner.run = None;
            if had_run {
                inner.state = Some(ClientLinkState::NeedsAttention {
                    reason: "reconnecting was stopped; leave the network or try again".into(),
                });
            }
            had_run
        };
        if notify {
            self.publish_current();
        }
    }

    /// The leave finished: this node is no longer a client, so there is no state to report.
    pub fn cleared(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.generation += 1;
            if let Some(run) = &inner.run {
                run.stop.store(true, Ordering::SeqCst);
            }
            inner.run = None;
            inner.state = None;
        }
        self.publish_current();
    }

    fn is_client(&self) -> Result<bool, CoreError> {
        let registry = self.memberships.load()?;
        let Some(network) = current_network(&registry)? else {
            return Ok(false);
        };
        Ok(registry.network(&network).is_some_and(|membership| {
            membership.state == MembershipState::Member
                && membership.role == Some(MembershipRole::Client)
        }))
    }

    fn publish_current(&self) {
        let state = self.state();
        self.publish(state.as_ref());
    }

    fn publish(&self, state: Option<&ClientLinkState>) {
        let observer = self.observer.lock().ok().and_then(|slot| slot.clone());
        if let Some(observer) = observer {
            observer(state);
        }
    }

    fn set_state(&self, generation: u64, state: ClientLinkState) {
        {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            if inner.generation != generation {
                return;
            }
            inner.state = Some(state.clone());
        }
        self.publish(Some(&state));
    }

    fn begin(self: &Arc<Self>) -> Result<(), CoreError> {
        if !self.is_client()? {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "this node is not a joined client, so there is nothing to reconnect",
            ));
        }
        let (generation, stop, done) = {
            let mut inner = self.inner.lock().map_err(|_| lock_error())?;
            if inner
                .run
                .as_ref()
                .is_some_and(|run| !run.done.load(Ordering::SeqCst))
            {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "BlueRoute is already trying to reconnect",
                ));
            }
            inner.generation += 1;
            let stop = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            inner.run = Some(Run {
                stop: stop.clone(),
                done: done.clone(),
                thread: None,
            });
            inner.state = Some(ClientLinkState::Connecting { attempt: 1 });
            (inner.generation, stop, done)
        };
        self.publish_current();
        let worker = Arc::clone(self);
        let worker_stop = stop.clone();
        let worker_done = done.clone();
        let spawned = thread::Builder::new()
            .name("blueroute-client-reconnect".to_owned())
            .spawn(move || {
                worker.run(generation, &worker_stop);
                worker_done.store(true, Ordering::SeqCst);
            });
        match spawned {
            Ok(handle) => {
                if let Ok(mut inner) = self.inner.lock()
                    && inner.generation == generation
                    && let Some(run) = &mut inner.run
                {
                    run.thread = Some(handle);
                }
                Ok(())
            }
            Err(error) => {
                done.store(true, Ordering::SeqCst);
                let reason = "BlueRoute could not start reconnecting".to_owned();
                self.set_state(generation, ClientLinkState::NeedsAttention { reason });
                Err(CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "failed to start the reconnection worker",
                    error.to_string(),
                ))
            }
        }
    }

    fn run(&self, generation: u64, stop: &Arc<AtomicBool>) {
        let attention = |reason: String| {
            self.set_state(generation, ClientLinkState::NeedsAttention { reason });
        };
        let prepared = self.prepare();
        let (network, host, hint, expected_lease) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return attention(error.message().to_owned()),
        };
        let runtime = CancellableRuntime {
            inner: self.runtime.clone(),
            stop: stop.clone(),
            supervisors: self.supervisors.clone(),
        };
        let sleeper = self.sleeper.clone();
        let end = futures_lite::future::block_on(run_reconnect(
            &runtime,
            network,
            host,
            hint,
            expected_lease,
            self.policy,
            stop,
            |delay| {
                sleeper(delay);
                async {}
            },
            &|progress| {
                let state = match progress {
                    ReconnectProgress::Attempting { attempt } => {
                        ClientLinkState::Connecting { attempt: *attempt }
                    }
                    ReconnectProgress::Waiting { attempt, delay, .. } => ClientLinkState::Waiting {
                        attempt: *attempt,
                        retry_in_seconds: delay.as_secs(),
                    },
                };
                self.set_state(generation, state);
            },
        ));
        match end {
            ReconnectEnd::Started { .. } => {
                self.set_state(generation, ClientLinkState::Established)
            }
            ReconnectEnd::Degraded(error) => attention(error.message().to_owned()),
            ReconnectEnd::GaveUp { attempts, .. } => {
                self.set_state(generation, ClientLinkState::GaveUp { attempts });
            }
            // Cancelled: `cancel` already published the state.
            ReconnectEnd::Stopped => {}
        }
    }

    #[allow(clippy::type_complexity)]
    fn prepare(
        &self,
    ) -> Result<
        (
            NetworkId,
            ClientHost,
            Option<PeerHandle>,
            blueroute_core::IpPrefix,
        ),
        CoreError,
    > {
        let registry = self.memberships.load()?;
        let network = current_network(&registry)?.ok_or_else(|| {
            CoreError::new(ErrorKind::InvalidState, "this node is not in a network")
        })?;
        let membership = registry.network(&network).ok_or_else(|| {
            CoreError::new(
                ErrorKind::InvalidState,
                "joined network is missing from membership",
            )
        })?;
        let host = single_approved_host(membership, &self.pins.load()?)?;
        // The recorded device path is only a hint; an unreadable record just means no hint.
        let hint = self
            .records
            .load_for(network)
            .ok()
            .flatten()
            .and_then(|record| record.device_hint().cloned());
        let expected = Ipv4StarAddressPlan::for_network(network, self.pool)?.first_client;
        Ok((network, host, hint, expected))
    }

    #[cfg(test)]
    fn wait_for_run(&self) {
        let handle = self
            .inner
            .lock()
            .ok()
            .and_then(|mut inner| inner.run.as_mut().and_then(|run| run.thread.take()));
        if let Some(handle) = handle {
            handle.join().unwrap();
        }
    }
}

/// Wraps the real runtime so a cancelled run can never start a session after a leave.
struct CancellableRuntime {
    inner: Arc<dyn ClientRuntime>,
    stop: Arc<AtomicBool>,
    supervisors: Arc<ControlSupervisors>,
}

impl ClientRuntime for CancellableRuntime {
    fn observe(
        &self,
        network: NetworkId,
        hint: Option<PeerHandle>,
    ) -> BackendFuture<'_, ObservedClient> {
        self.inner.observe(network, hint)
    }

    fn lease_segment_free(&self, network: NetworkId) -> BackendFuture<'_, bool> {
        self.inner.lease_segment_free(network)
    }

    fn reestablish_panu(
        &self,
        network: NetworkId,
        hint: Option<PeerHandle>,
    ) -> BackendFuture<'_, JoinPanLink> {
        self.inner.reestablish_panu(network, hint)
    }

    fn apply_lease(
        &self,
        network: NetworkId,
        link: &JoinPanLink,
    ) -> BackendFuture<'_, JoinIpLease> {
        self.inner.apply_lease(network, link)
    }

    fn start_supervisor(
        &self,
        network: NetworkId,
        host: ClientHost,
        link: JoinPanLink,
        lease: JoinIpLease,
    ) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let cancelled = || {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "reconnecting was cancelled before the session started",
                )
            };
            if self.stop.load(Ordering::SeqCst) {
                return Err(cancelled());
            }
            self.inner
                .start_supervisor(network, host, link, lease)
                .await?;
            // A leave may have cancelled while the session was starting: take it down again.
            if self.stop.load(Ordering::SeqCst) {
                let _ = self.supervisors.stop(network);
                return Err(cancelled());
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    use blueroute_core::{DisplayName, IpPrefix, MembershipRegistry, NetworkMembership, NodeId};
    use blueroute_linux::{InterfaceAddress, NetworkInterfaceHandle, PanAttachment, PanRole};
    use blueroute_protocol::peer_pin::{PeerKeyPins, SpkiSha256};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn net() -> NetworkId {
        NetworkId::from_bytes([0x81; 16])
    }

    fn host_node() -> NodeId {
        NodeId::from_bytes([0x82; 16])
    }

    fn peer() -> PeerHandle {
        PeerHandle::new("/org/bluez/hci0/dev_AA_00_00_00_00_01").unwrap()
    }

    /// Each `observe` pops the next scripted result; a link that is up with its owned lease
    /// present leads straight to the supervisor, so the script alone drives the outcome.
    struct Scripted {
        script: Mutex<VecDeque<Result<(), CoreError>>>,
        supervisor_starts: AtomicUsize,
        pool: Ipv4AddressPool,
    }

    impl Scripted {
        fn new(script: Vec<Result<(), CoreError>>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into()),
                supervisor_starts: AtomicUsize::new(0),
                pool: Ipv4AddressPool::default(),
            })
        }
    }

    impl ClientRuntime for Scripted {
        fn observe(
            &self,
            network: NetworkId,
            _hint: Option<PeerHandle>,
        ) -> BackendFuture<'_, ObservedClient> {
            Box::pin(async move {
                let next = self.script.lock().unwrap().pop_front().unwrap_or(Ok(()));
                let prefix = Ipv4StarAddressPlan::for_network(network, self.pool)
                    .unwrap()
                    .first_client;
                next.map(|()| ObservedClient {
                    panu: Some(PanAttachment {
                        role: PanRole::Panu,
                        interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
                        peer: Some(peer()),
                    }),
                    owned_addresses: vec![InterfaceAddress {
                        interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
                        prefix,
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
            Box::pin(async move {
                self.supervisor_starts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    struct Fixture {
        root: PathBuf,
        reconnector: Arc<ClientReconnector>,
        runtime: Arc<Scripted>,
        seen: Arc<Mutex<Vec<Option<ClientLinkState>>>>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(
        role: Option<MembershipRole>,
        pinned: bool,
        script: Vec<Result<(), CoreError>>,
        max_attempts: u32,
    ) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "blueroute-client-link-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let memberships = NetworkMembershipStore::new(root.join("memberships"));
        let mut membership = NetworkMembership::new(net(), DisplayName::new("Joined").unwrap());
        membership.state = MembershipState::Member;
        membership.role = role;
        membership.trust_peer(host_node());
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        memberships.save(&registry).unwrap();

        let pin_store = PeerPinStore::new(root.join("pins"));
        if pinned {
            let mut pins = PeerKeyPins::new();
            let key = SpkiSha256::from_verified_certificate_digest([0x83; 32]).unwrap();
            pins.enroll_verified(net(), host_node(), key).unwrap();
            pin_store.save(&pins).unwrap();
        }
        let runtime = Scripted::new(script);
        let reconnector = ClientReconnector::with_sleeper(
            memberships,
            pin_store,
            Arc::new(ClientRecordStore::new(root.join("client"))),
            runtime.clone(),
            Arc::new(ControlSupervisors::new()),
            Ipv4AddressPool::default(),
            ReconnectPolicy {
                max_attempts,
                initial_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(1),
            },
            Arc::new(|_| {}),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        reconnector.set_observer(Arc::new(move |state| {
            sink.lock().unwrap().push(state.cloned());
        }));
        Fixture {
            root,
            reconnector,
            runtime,
            seen,
        }
    }

    fn transient() -> Result<(), CoreError> {
        Err(CoreError::new(ErrorKind::PanFailure, "host not in range"))
    }

    #[test]
    fn health_follows_the_published_state() {
        let health = |state: Option<ClientLinkState>| health_for_client_link(state.as_ref());
        assert_eq!(health(None), HealthLevel::Healthy);
        assert_eq!(
            health(Some(ClientLinkState::Established)),
            HealthLevel::Healthy
        );
        assert_eq!(
            health(Some(ClientLinkState::Connecting { attempt: 1 })),
            HealthLevel::Reconnecting
        );
        assert_eq!(
            health(Some(ClientLinkState::Waiting {
                attempt: 1,
                retry_in_seconds: 2
            })),
            HealthLevel::Reconnecting
        );
        assert_eq!(
            health(Some(ClientLinkState::GaveUp { attempts: 3 })),
            HealthLevel::Degraded
        );
        assert_eq!(
            health(Some(ClientLinkState::NeedsAttention { reason: "x".into() })),
            HealthLevel::Degraded
        );
    }

    #[test]
    fn a_host_or_a_node_with_no_role_is_never_reconnected_as_a_client() {
        for role in [Some(MembershipRole::Host), None] {
            let fx = fixture(role, true, vec![], 3);
            assert!(!fx.reconnector.start().unwrap());
            assert_eq!(fx.reconnector.state(), None);
            assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 0);
            assert_eq!(
                fx.reconnector.retry().unwrap_err().kind(),
                ErrorKind::InvalidState
            );
        }
    }

    #[test]
    fn a_persisted_client_reconnects_in_the_background_and_reports_established() {
        let fx = fixture(
            Some(MembershipRole::Client),
            true,
            vec![transient(), Ok(())],
            5,
        );
        assert!(fx.reconnector.start().unwrap());
        fx.reconnector.wait_for_run();
        assert_eq!(fx.reconnector.state(), Some(ClientLinkState::Established));
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 1);
        let seen = fx.seen.lock().unwrap();
        assert_eq!(
            seen.first(),
            Some(&Some(ClientLinkState::Connecting { attempt: 1 }))
        );
        assert!(seen.contains(&Some(ClientLinkState::Waiting {
            attempt: 1,
            retry_in_seconds: 1
        })));
        assert_eq!(seen.last(), Some(&Some(ClientLinkState::Established)));
    }

    #[test]
    fn it_gives_up_after_the_budget_and_the_user_can_retry() {
        let fx = fixture(
            Some(MembershipRole::Client),
            true,
            vec![transient(), transient(), Ok(())],
            2,
        );
        fx.reconnector.start().unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(
            fx.reconnector.state(),
            Some(ClientLinkState::GaveUp { attempts: 2 })
        );
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 0);
        // The user asks again; the script now succeeds.
        fx.reconnector.retry().unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(fx.reconnector.state(), Some(ClientLinkState::Established));
    }

    #[test]
    fn a_second_retry_while_one_is_running_is_refused() {
        let fx = fixture(Some(MembershipRole::Client), true, vec![Ok(())], 3);
        // Hold the run "active" by never letting the first one finish before the second call.
        {
            let mut inner = fx.reconnector.inner.lock().unwrap();
            inner.run = Some(Run {
                stop: Arc::new(AtomicBool::new(false)),
                done: Arc::new(AtomicBool::new(false)),
                thread: None,
            });
        }
        assert_eq!(
            fx.reconnector.retry().unwrap_err().kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn something_that_needs_a_person_is_reported_in_plain_words_and_not_retried() {
        let fx = fixture(
            Some(MembershipRole::Client),
            true,
            vec![Err(CoreError::with_diagnostic(
                ErrorKind::AuthenticationFailed,
                "the host key did not match",
                "raw: spki mismatch deadbeef",
            ))],
            5,
        );
        fx.reconnector.start().unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(
            fx.reconnector.state(),
            Some(ClientLinkState::NeedsAttention {
                reason: "the host key did not match".into()
            })
        );
    }

    #[test]
    fn a_client_with_no_approved_host_key_needs_attention_and_does_not_dial() {
        let fx = fixture(Some(MembershipRole::Client), false, vec![Ok(())], 3);
        fx.reconnector.start().unwrap();
        fx.reconnector.wait_for_run();
        assert!(matches!(
            fx.reconnector.state(),
            Some(ClientLinkState::NeedsAttention { .. })
        ));
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancel_ignores_a_late_result_and_cleared_reports_nothing() {
        let fx = fixture(Some(MembershipRole::Client), true, vec![Ok(())], 3);
        fx.reconnector.start().unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(fx.reconnector.state(), Some(ClientLinkState::Established));
        fx.reconnector.cancel();
        assert!(matches!(
            fx.reconnector.state(),
            Some(ClientLinkState::NeedsAttention { .. })
        ));
        fx.reconnector.cleared();
        assert_eq!(fx.reconnector.state(), None);
        assert_eq!(fx.seen.lock().unwrap().last(), Some(&None));
    }

    #[test]
    fn a_cancelled_run_is_refused_when_it_tries_to_start_a_session() {
        let fx = fixture(Some(MembershipRole::Client), true, vec![], 3);
        let stop = Arc::new(AtomicBool::new(true));
        let guarded = CancellableRuntime {
            inner: fx.runtime.clone(),
            stop,
            supervisors: Arc::new(ControlSupervisors::new()),
        };
        let link = JoinPanLink::new(
            peer(),
            PanAttachment {
                role: PanRole::Panu,
                interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
                peer: Some(peer()),
            },
        )
        .unwrap();
        let lease = JoinIpLease::new(InterfaceAddress {
            interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
            prefix: IpPrefix::new("192.168.77.2".parse().unwrap(), 24).unwrap(),
            owner: net(),
        });
        let host = ClientHost {
            node: host_node(),
            fingerprint: SpkiSha256::from_verified_certificate_digest([0x83; 32]).unwrap(),
        };
        let error =
            futures_lite::future::block_on(guarded.start_supervisor(net(), host, link, lease))
                .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn published_health_is_the_worse_of_the_capability_monitor_and_the_reconnection() {
        use crate::DaemonService;
        let fx = fixture(
            Some(MembershipRole::Client),
            true,
            vec![transient(), transient(), Ok(())],
            2,
        );
        let service = DaemonService::new(
            NodeId::from_bytes([0x84; 16]),
            HealthLevel::Healthy,
            blueroute_core::NodeCapabilities::default(),
        )
        .with_client_reconnector(fx.reconnector.clone());
        let health = || service.status_snapshot().unwrap().health;
        assert_eq!(health(), HealthLevel::Healthy);

        // Gave up: Degraded, and a Healthy reading from the capability monitor cannot hide it.
        fx.reconnector.start().unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(health(), HealthLevel::Degraded);
        service.update_runtime_health(HealthLevel::Healthy).unwrap();
        assert_eq!(health(), HealthLevel::Degraded);

        // Established: the capability reading shows through again, including when it is worse.
        fx.reconnector.retry().unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(health(), HealthLevel::Healthy);
        service.update_runtime_health(HealthLevel::Error).unwrap();
        assert_eq!(health(), HealthLevel::Error);

        // A leave clears the client's contribution.
        fx.reconnector.cleared();
        service.update_runtime_health(HealthLevel::Healthy).unwrap();
        assert_eq!(health(), HealthLevel::Healthy);
    }
}

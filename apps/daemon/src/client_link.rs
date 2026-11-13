//! Restart and link-loss reconnection of a joined client as a daemon service (P6-009 S5b,
//! P6-010).
//!
//! At startup, a persisted `Client` is reconnected in the **background** so a slow radio can
//! never block daemon start or D-Bus. Each reconnect is bounded (see `ReconnectPolicy`): it ends
//! in `Established`, `NeedsAttention` or `GaveUp`, never retries forever, and never touches
//! durable membership. The user can start a fresh run with `retry`. Leave cancels the run first.
//!
//! Once `Established` (after a reconnect, or after a fresh join via `joined`), the same run keeps
//! **watching** the supervised control session. When the session ends by itself (PANU or TLS
//! lost, idle timeout) it collects the worker and starts a fresh bounded reconnect (P6-010), so
//! a lost host is reported as `Reconnecting` and health returns once it is back.
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
    SupervisedState, current_network, run_reconnect, single_approved_host,
};

/// How often an established run checks whether its control session is still alive.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// What a run does first.
#[derive(Clone, Copy)]
enum Phase {
    /// Reconnect (startup, user retry, or after a loss).
    Reconnect,
    /// The session for this network is already up (a fresh join): only watch it.
    Watch(NetworkId),
}

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
    watch_interval: Duration,
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
            WATCH_INTERVAL,
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
        watch_interval: Duration,
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
            watch_interval,
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
        match self.begin(Phase::Reconnect) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::InvalidState && !self.is_client()? => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// The user asked to try again. Refused unless this is a joined client with no run active.
    pub fn retry(self: &Arc<Self>) -> Result<(), CoreError> {
        self.begin(Phase::Reconnect)
    }

    /// A join for `network` just succeeded with its session supervised: report `Established`
    /// and watch the session so a later link loss is reconnected (P6-010).
    pub fn joined(self: &Arc<Self>, network: NetworkId) -> Result<(), CoreError> {
        self.begin(Phase::Watch(network))
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

    fn is_current(&self, generation: u64) -> bool {
        self.inner
            .lock()
            .is_ok_and(|inner| inner.generation == generation)
    }

    fn begin(self: &Arc<Self>, phase: Phase) -> Result<(), CoreError> {
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
                let message = if inner.state == Some(ClientLinkState::Established) {
                    "already connected to the host"
                } else {
                    "BlueRoute is already trying to reconnect"
                };
                return Err(CoreError::new(ErrorKind::InvalidState, message));
            }
            inner.generation += 1;
            let stop = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            inner.run = Some(Run {
                stop: stop.clone(),
                done: done.clone(),
                thread: None,
            });
            inner.state = Some(match phase {
                Phase::Reconnect => ClientLinkState::Connecting { attempt: 1 },
                Phase::Watch(_) => ClientLinkState::Established,
            });
            (inner.generation, stop, done)
        };
        self.publish_current();
        let worker = Arc::clone(self);
        let worker_stop = stop.clone();
        let worker_done = done.clone();
        let spawned = thread::Builder::new()
            .name("blueroute-client-reconnect".to_owned())
            .spawn(move || {
                worker.run(generation, &worker_stop, phase);
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

    fn run(&self, generation: u64, stop: &Arc<AtomicBool>, mut phase: Phase) {
        loop {
            let network = match phase {
                Phase::Reconnect => match self.reconnect(generation, stop) {
                    Some(network) => network,
                    None => return,
                },
                Phase::Watch(network) => network,
            };
            match self.watch(generation, stop, network) {
                Some(reason) => {
                    eprintln!(
                        "BlueRoute lost the control session to its host ({reason}); reconnecting"
                    );
                    phase = Phase::Reconnect;
                }
                None => return,
            }
        }
    }

    /// Wait while the session for `network` runs. `Some(reason)` when it ended by itself (the
    /// worker is collected first); `None` when this run was cancelled or superseded, or nothing
    /// is supervised for the network any more (a leave stopped it), so there is nothing to do.
    fn watch(&self, generation: u64, stop: &AtomicBool, network: NetworkId) -> Option<String> {
        loop {
            if stop.load(Ordering::SeqCst) || !self.is_current(generation) {
                return None;
            }
            match self.supervisors.state(network) {
                None => return None,
                Some(SupervisedState::Ended(reason)) => {
                    let _ = self.supervisors.stop(network);
                    if stop.load(Ordering::SeqCst) || !self.is_current(generation) {
                        return None;
                    }
                    return Some(reason.unwrap_or_else(|| "the session ended".to_owned()));
                }
                Some(SupervisedState::Establishing | SupervisedState::Running) => {}
            }
            thread::sleep(self.watch_interval);
        }
    }

    /// One bounded reconnect. `Some(network)` when the session is established.
    fn reconnect(&self, generation: u64, stop: &Arc<AtomicBool>) -> Option<NetworkId> {
        let attention = |reason: String| {
            self.set_state(generation, ClientLinkState::NeedsAttention { reason });
        };
        let prepared = self.prepare();
        let (network, host, hint, expected_lease) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                attention(error.message().to_owned());
                return None;
            }
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
                self.set_state(generation, ClientLinkState::Established);
                return Some(network);
            }
            ReconnectEnd::Degraded(error) => attention(error.message().to_owned()),
            ReconnectEnd::GaveUp { attempts, .. } => {
                self.set_state(generation, ClientLinkState::GaveUp { attempts });
            }
            // Cancelled: `cancel` already published the state.
            ReconnectEnd::Stopped => {}
        }
        None
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
    fn take_run_thread(&self) -> Option<JoinHandle<()>> {
        self.inner
            .lock()
            .ok()
            .and_then(|mut inner| inner.run.as_mut().and_then(|run| run.thread.take()))
    }

    #[cfg(test)]
    fn wait_for_run(&self) {
        if let Some(handle) = self.take_run_thread() {
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
    /// With `live` set, `start_supervisor` registers a real supervised worker that runs until it
    /// is stopped or `lose` is raised, which ends it by itself like a lost PANU link.
    struct Scripted {
        script: Mutex<VecDeque<Result<(), CoreError>>>,
        supervisor_starts: AtomicUsize,
        pool: Ipv4AddressPool,
        live: Option<Arc<ControlSupervisors>>,
        lose: Arc<AtomicBool>,
    }

    impl Scripted {
        fn new(
            script: Vec<Result<(), CoreError>>,
            live: Option<Arc<ControlSupervisors>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into()),
                supervisor_starts: AtomicUsize::new(0),
                pool: Ipv4AddressPool::default(),
                live,
                lose: Arc::new(AtomicBool::new(false)),
            })
        }
    }

    fn supervise_until_lost(supervisors: &ControlSupervisors, lose: Arc<AtomicBool>) {
        futures_lite::future::block_on(supervisors.start(net(), move |stop, established| {
            established.report(Ok(()));
            while !stop.load(Ordering::Acquire) {
                if lose.swap(false, Ordering::SeqCst) {
                    return Err(CoreError::new(ErrorKind::PanFailure, "PANU link lost"));
                }
                thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        }))
        .unwrap();
    }

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                std::time::Instant::now() < deadline,
                "condition not reached in time"
            );
            thread::sleep(Duration::from_millis(2));
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
                if let Some(supervisors) = &self.live {
                    supervise_until_lost(supervisors, self.lose.clone());
                }
                Ok(())
            })
        }
    }

    struct Fixture {
        root: PathBuf,
        reconnector: Arc<ClientReconnector>,
        runtime: Arc<Scripted>,
        supervisors: Arc<ControlSupervisors>,
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
        fixture_with(role, pinned, script, max_attempts, false)
    }

    /// A joined client whose supervised sessions are real workers that a test can end.
    fn live_fixture(script: Vec<Result<(), CoreError>>, max_attempts: u32) -> Fixture {
        fixture_with(
            Some(MembershipRole::Client),
            true,
            script,
            max_attempts,
            true,
        )
    }

    fn fixture_with(
        role: Option<MembershipRole>,
        pinned: bool,
        script: Vec<Result<(), CoreError>>,
        max_attempts: u32,
        live: bool,
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
        let supervisors = Arc::new(ControlSupervisors::new());
        let runtime = Scripted::new(script, live.then(|| supervisors.clone()));
        let reconnector = ClientReconnector::with_sleeper(
            memberships,
            pin_store,
            Arc::new(ClientRecordStore::new(root.join("client"))),
            runtime.clone(),
            supervisors.clone(),
            Ipv4AddressPool::default(),
            ReconnectPolicy {
                max_attempts,
                initial_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(1),
            },
            Arc::new(|_| {}),
            Duration::from_millis(2),
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
            supervisors,
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

    #[test]
    fn every_link_phase_is_announced_even_when_health_does_not_change() {
        use crate::DaemonService;
        // Connecting 1 → Waiting 1 → Connecting 2 → Gave up: health only moves Healthy →
        // Reconnecting → Degraded, yet observers must reread at each phase (CR-F1-05).
        let fx = fixture(
            Some(MembershipRole::Client),
            true,
            vec![transient(), transient()],
            2,
        );
        let service = DaemonService::new(
            NodeId::from_bytes([0x85; 16]),
            HealthLevel::Healthy,
            blueroute_core::NodeCapabilities::default(),
        )
        .with_client_reconnector(fx.reconnector.clone());
        let publisher = service.state_publisher();
        let before = service.observable_state().unwrap();
        fx.reconnector.start().unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(
            fx.reconnector.state(),
            Some(ClientLinkState::GaveUp { attempts: 2 })
        );
        assert!(
            publisher.marks() >= 4,
            "only {} marks for four phases",
            publisher.marks()
        );
        let after = service.observable_state().unwrap();
        assert_ne!(before, after);
        assert!(after.contains("gave_up"), "{after}");
    }

    fn established_after(fx: &Fixture, supervisor_starts: usize) {
        until(|| {
            fx.runtime.supervisor_starts.load(Ordering::SeqCst) == supervisor_starts
                && fx.reconnector.state() == Some(ClientLinkState::Established)
                && fx.supervisors.state(net()) == Some(SupervisedState::Running)
        });
    }

    #[test]
    fn a_lost_session_is_reconnected_and_health_returns_when_the_host_is_back() {
        // Established, then the link is lost; the first reconnect attempt fails, the next works.
        let fx = live_fixture(vec![Ok(()), transient(), Ok(())], 5);
        fx.reconnector.start().unwrap();
        established_after(&fx, 1);
        let established_at = fx.seen.lock().unwrap().len();

        fx.runtime.lose.store(true, Ordering::SeqCst);
        established_after(&fx, 2);
        let seen = fx.seen.lock().unwrap()[established_at..].to_vec();
        assert_eq!(
            seen.first(),
            Some(&Some(ClientLinkState::Connecting { attempt: 1 }))
        );
        assert!(seen.contains(&Some(ClientLinkState::Waiting {
            attempt: 1,
            retry_in_seconds: 1
        })));
        assert_eq!(seen.last(), Some(&Some(ClientLinkState::Established)));

        // A leave: the watcher ends and nothing is reconnected afterwards.
        let thread = fx.reconnector.take_run_thread().unwrap();
        fx.reconnector.cancel();
        thread.join().unwrap();
        fx.supervisors.stop(net()).unwrap();
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_loss_that_keeps_failing_gives_up_and_waits_for_the_user() {
        let fx = live_fixture(vec![Ok(()), transient(), transient(), Ok(())], 2);
        fx.reconnector.start().unwrap();
        established_after(&fx, 1);
        fx.runtime.lose.store(true, Ordering::SeqCst);
        // The run ends after its budget; the lost worker was collected, not left registered.
        fx.reconnector.wait_for_run();
        assert_eq!(
            fx.reconnector.state(),
            Some(ClientLinkState::GaveUp { attempts: 2 })
        );
        assert_eq!(fx.supervisors.state(net()), None);
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 1);
        // The user retries once the host is back.
        fx.reconnector.retry().unwrap();
        established_after(&fx, 2);
        let thread = fx.reconnector.take_run_thread().unwrap();
        fx.reconnector.cleared();
        thread.join().unwrap();
        fx.supervisors.stop(net()).unwrap();
    }

    #[test]
    fn a_fresh_join_is_watched_without_dialing_again() {
        let fx = live_fixture(vec![Ok(())], 3);
        // The join supervised its session itself.
        supervise_until_lost(&fx.supervisors, fx.runtime.lose.clone());
        fx.reconnector.joined(net()).unwrap();
        assert_eq!(fx.reconnector.state(), Some(ClientLinkState::Established));
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 0);
        // Retrying a working link is refused rather than starting a second session.
        let refused = fx.reconnector.retry().unwrap_err();
        assert_eq!(refused.kind(), ErrorKind::InvalidState);
        assert!(refused.message().contains("already connected"));

        fx.runtime.lose.store(true, Ordering::SeqCst);
        established_after(&fx, 1);
        let thread = fx.reconnector.take_run_thread().unwrap();
        fx.reconnector.cleared();
        thread.join().unwrap();
        fx.supervisors.stop(net()).unwrap();
    }

    #[test]
    fn a_session_stopped_by_a_leave_is_not_treated_as_a_loss() {
        let fx = live_fixture(vec![Ok(())], 3);
        fx.reconnector.start().unwrap();
        established_after(&fx, 1);
        // The leave's teardown stops the supervisor; the watcher sees nothing supervised and ends.
        fx.supervisors.stop(net()).unwrap();
        fx.reconnector.wait_for_run();
        assert_eq!(fx.runtime.supervisor_starts.load(Ordering::SeqCst), 1);
        assert_eq!(fx.reconnector.state(), Some(ClientLinkState::Established));
    }
}

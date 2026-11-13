use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use blueroute_core::{
    CoreError, DisplayName, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, MembershipRegistry,
    MembershipRole, MembershipState, NetworkId, NetworkMembership, ensure_ipv4_segment_available,
};
use blueroute_linux::{
    BluezBackend, InterfaceAddress, IpNetworkBackend, IpNetworkObservationBackend,
    NetworkInterfaceHandle, NetworkManagerBackend, NetworkMembershipStore, NetworkStateBackend,
    PanAttachment, PanBackend, PanRole, PeerHandle,
};

use crate::current_network;
use crate::{
    BluezPeerLocator, ClientRecord, ClientRecordStore, ControlSupervisors, JoinCommitAuthorization,
    JoinControlConfig, PeerLocator, PendingJoinOperations, PinnedControlTlsPeer, SystemClientLeave,
    SystemClientRuntime, leave_observed, run_join_control,
};

/// Links this process created, kept only as an optimization (leave and reconnect re-observe).
pub(crate) type RetainedLinks =
    Arc<std::sync::Mutex<std::collections::HashMap<NetworkId, PanAttachment>>>;

pub type JoinNetworkFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

pub trait JoinNetworkOperations: Send + Sync {
    fn join_network(&self, network: NetworkId) -> JoinNetworkFuture<'_, ()>;

    /// Leave as a **joined client** if this daemon joined its current network through
    /// `join_network` in this process. Returns `Ok(false)` when it is not such a client (no
    /// network, or a host, or a restart that lost the runtime state), so the caller can fall back
    /// to host leave. Reverses the join: stop the control session, remove the owned IP, disconnect
    /// PANU, then clear membership. A teardown failure keeps `Member` so leave can be retried.
    fn leave_joined(&self) -> JoinNetworkFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinPanLink {
    peer: PeerHandle,
    attachment: PanAttachment,
}

impl JoinPanLink {
    pub fn new(peer: PeerHandle, attachment: PanAttachment) -> Result<Self, CoreError> {
        if attachment.role != PanRole::Panu || attachment.peer.as_ref() != Some(&peer) {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "PANU join runtime returned an attachment for the wrong peer or role",
            ));
        }
        Ok(Self { peer, attachment })
    }

    pub fn interface(&self) -> &NetworkInterfaceHandle {
        &self.attachment.interface
    }

    /// Preserve the original BlueZ attachment identity for live pre-dial
    /// revalidation; a network interface name alone is not PANU ownership.
    pub fn attachment(&self) -> &PanAttachment {
        &self.attachment
    }

    pub fn peer(&self) -> &PeerHandle {
        &self.peer
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinIpLease {
    address: InterfaceAddress,
}

impl JoinIpLease {
    pub fn new(address: InterfaceAddress) -> Self {
        Self { address }
    }

    pub fn interface(&self) -> &NetworkInterfaceHandle {
        &self.address.interface
    }

    pub fn address(&self) -> &InterfaceAddress {
        &self.address
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinControlSession {
    network: NetworkId,
}

impl JoinControlSession {
    pub const fn new(network: NetworkId) -> Self {
        Self { network }
    }

    pub const fn network(&self) -> NetworkId {
        self.network
    }
}

pub trait JoinRuntime: Send + Sync {
    fn preflight(&self, network: NetworkId) -> JoinNetworkFuture<'_, ()>;
    fn establish_panu(&self, network: NetworkId) -> JoinNetworkFuture<'_, JoinPanLink>;
    fn disconnect_panu(&self, link: JoinPanLink) -> JoinNetworkFuture<'_, ()>;
    fn configure_ip<'a>(
        &'a self,
        network: NetworkId,
        link: &'a JoinPanLink,
    ) -> JoinNetworkFuture<'a, JoinIpLease>;
    fn remove_ip(&self, lease: JoinIpLease) -> JoinNetworkFuture<'_, ()>;
    fn start_control_session<'a>(
        &'a self,
        network: NetworkId,
        link: &'a JoinPanLink,
        lease: &'a JoinIpLease,
    ) -> JoinNetworkFuture<'a, JoinControlSession>;
    fn stop_control_session(&self, session: JoinControlSession) -> JoinNetworkFuture<'_, ()>;

    /// A join committed: retain the live link as an optimization. Leave never depends on it.
    fn adopt_joined(&self, _network: NetworkId, _link: JoinPanLink, _lease: JoinIpLease) {}

    /// Reverse a committed join from observation: stop the control session, disconnect the PANU
    /// link and remove this network's owned IP configuration, using ownership metadata rather
    /// than remembered handles (`hint` is the recorded host device path). Idempotent, so a
    /// failed attempt can be retried.
    fn leave_joined(
        &self,
        _network: NetworkId,
        _hint: Option<PeerHandle>,
    ) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    /// The durable membership was cleared; drop the retained state.
    fn forget_joined(&self, _network: NetworkId) {}
}

/// Serialize join attempts. Cancellation before runtime mutation is safe;
/// cancellation after mutation may strand a PANU/IP lease or control session.
/// Without proven cleanup, reserve this coordinator until an explicit
/// reconciliation path exists (P6-009). Dropping an async future is not rollback.
struct JoinTransitionGuard<'a> {
    flag: &'a AtomicBool,
    may_have_live_resources: bool,
}

impl<'a> JoinTransitionGuard<'a> {
    fn claim(flag: &'a AtomicBool) -> Result<Self, CoreError> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "another BlueRoute join operation is already in progress or requires runtime reconciliation",
                )
            })?;
        Ok(Self {
            flag,
            may_have_live_resources: false,
        })
    }

    fn mutation_started(&mut self) {
        self.may_have_live_resources = true;
    }

    fn reconciled(&mut self) {
        self.may_have_live_resources = false;
    }
}

impl Drop for JoinTransitionGuard<'_> {
    fn drop(&mut self) {
        if !self.may_have_live_resources {
            self.flag.store(false, Ordering::Release);
        }
    }
}

pub struct TransactionalJoinNetworkOperations<R = LinuxJoinRuntime> {
    store: NetworkMembershipStore,
    runtime: R,
    expected_ipv4_pool: Ipv4AddressPool,
    join_in_progress: AtomicBool,
    /// Binds the operator-confirmed host into durable member state at commit. `None` is only for
    /// tests with fake runtimes; production wiring must supply it before join is ever enabled.
    authorization: Option<Arc<dyn JoinCommitAuthorization>>,
    client_records: Option<Arc<ClientRecordStore>>,
}

/// Whether this binary was built to allow opening the join gate for hardware testing. A `cfg!`
/// constant, so every line around it is still compiled and linted in normal builds.
pub const EXPERIMENTAL_JOIN: bool = cfg!(feature = "experimental-join");

impl TransactionalJoinNetworkOperations<LinuxJoinRuntime> {
    /// Open the join gate for two-machine hardware testing (P6-004 D5). Refused unless the
    /// binary was built with `experimental-join` **and** everything join depends on is wired:
    /// the operator-confirmed host record, the control TLS configuration, the commit-time host
    /// authorization and the restart record store (P6-009). A build without the feature can
    /// only refuse, so packaged and CI builds stay closed.
    pub fn open_join_gate_for_hardware_testing(self) -> Result<Self, CoreError> {
        self.open_join_gate(EXPERIMENTAL_JOIN)
    }

    fn open_join_gate(mut self, built_with_feature: bool) -> Result<Self, CoreError> {
        let refuse = |why: &'static str| Err(CoreError::new(ErrorKind::CapabilityUnavailable, why));
        if !built_with_feature {
            return refuse("this daemon was not built with the experimental-join feature");
        }
        if self.runtime.pending.is_none() || self.runtime.control.is_none() {
            return refuse("join cannot open without the confirmed-host record and control TLS");
        }
        if self.authorization.is_none() {
            return refuse("join cannot open without commit-time host authorization");
        }
        if self.client_records.is_none() {
            return refuse("join cannot open without the restart record store");
        }
        self.runtime.hardware_verified = true;
        Ok(self)
    }

    pub fn new(store: NetworkMembershipStore) -> Self {
        Self::with_runtime(store, LinuxJoinRuntime::default())
    }

    pub fn with_ipv4_pool(
        store: NetworkMembershipStore,
        pool: Ipv4AddressPool,
        pending: Arc<PendingJoinOperations>,
        control: JoinControlConfig,
    ) -> Result<Self, CoreError> {
        let runtime = LinuxJoinRuntime::new(pool)?
            .with_pending_join(pending)
            .with_control(control);
        Ok(Self::with_runtime_and_pool(store, runtime, pool))
    }

    /// The restart-reconnection runtime sharing this coordinator's supervisors and gate.
    pub fn client_runtime(&self) -> Option<SystemClientRuntime> {
        self.runtime.client_runtime()
    }

    pub fn supervisors(&self) -> Arc<ControlSupervisors> {
        self.runtime.supervisors()
    }
}

impl<R> TransactionalJoinNetworkOperations<R>
where
    R: JoinRuntime,
{
    pub fn with_runtime(store: NetworkMembershipStore, runtime: R) -> Self {
        Self::with_runtime_and_pool(store, runtime, Ipv4AddressPool::default())
    }

    fn with_runtime_and_pool(
        store: NetworkMembershipStore,
        runtime: R,
        expected_ipv4_pool: Ipv4AddressPool,
    ) -> Self {
        Self {
            store,
            runtime,
            expected_ipv4_pool,
            join_in_progress: AtomicBool::new(false),
            authorization: None,
            client_records: None,
        }
    }

    /// Persist the reconnect hint (P6-009 S2) after a join commits and clear it on leave.
    pub fn with_client_records(mut self, records: Arc<ClientRecordStore>) -> Self {
        self.client_records = Some(records);
        self
    }

    pub fn with_authorization(mut self, authorization: Arc<dyn JoinCommitAuthorization>) -> Self {
        self.authorization = Some(authorization);
        self
    }

    async fn rollback_after_control(
        &self,
        primary: CoreError,
        session: JoinControlSession,
        lease: JoinIpLease,
        link: JoinPanLink,
    ) -> (CoreError, bool) {
        let control = self.runtime.stop_control_session(session).await;
        let address = self.runtime.remove_ip(lease).await;
        let panu = self.runtime.disconnect_panu(link).await;
        let clean = control.is_ok() && address.is_ok() && panu.is_ok();
        (combine_rollback(primary, control, address, panu), clean)
    }

    async fn rollback_after_ip(
        &self,
        primary: CoreError,
        lease: JoinIpLease,
        link: JoinPanLink,
    ) -> (CoreError, bool) {
        let address = self.runtime.remove_ip(lease).await;
        let panu = self.runtime.disconnect_panu(link).await;
        let clean = address.is_ok() && panu.is_ok();
        (combine_rollback(primary, Ok(()), address, panu), clean)
    }
}

impl<R> JoinNetworkOperations for TransactionalJoinNetworkOperations<R>
where
    R: JoinRuntime,
{
    fn join_network(&self, network: NetworkId) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async move {
            let mut runtime_mutated = false;
            let result = self.join_inner(network, &mut runtime_mutated).await;
            // After any failed attempt that touched the runtime, forget the operator's pending
            // host so stale intent cannot authorize a later attempt; retrying needs a fresh record.
            if let (Err(error), true, Some(authorization)) =
                (&result, runtime_mutated, &self.authorization)
                && let Err(discard) = authorization.discard(network)
            {
                return Err(CoreError::with_diagnostic(
                    error.kind(),
                    error.message(),
                    format!(
                        "{}; also failed to clear the pending join host: {discard}",
                        error.diagnostic().unwrap_or("no primary diagnostic")
                    ),
                ));
            }
            result
        })
    }

    fn leave_joined(&self) -> JoinNetworkFuture<'_, bool> {
        Box::pin(async move {
            // A leave must never overlap a join or another leave.
            let _transition = JoinTransitionGuard::claim(&self.join_in_progress)?;
            let mut registry = self.store.load()?;
            let Some(network) = current_network(&registry)? else {
                return Ok(false);
            };
            // The persisted role decides, never in-memory state, so a client that restarted
            // can still leave. Hosts and unknown roles are left to host leave.
            let is_client = registry.network(&network).is_some_and(|membership| {
                membership.state == MembershipState::Member
                    && membership.role == Some(MembershipRole::Client)
            });
            if !is_client {
                return Ok(false);
            }
            // The recorded host device path is only a hint; a missing or unreadable record
            // just means the live link is found through the retained handle or not at all.
            let hint = self
                .client_records
                .as_ref()
                .and_then(|records| records.load_for(network).ok().flatten())
                .and_then(|record| record.device_hint().cloned());
            // A failure keeps `Member` so the user can retry; nothing is cleared that was not
            // proven torn down.
            self.runtime.leave_joined(network, hint).await?;
            let membership = registry.network_mut(&network).ok_or_else(|| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "joined BlueRoute membership changed during leave",
                )
            })?;
            membership.state = membership
                .state
                .transition(MembershipState::Leaving)?
                .transition(MembershipState::NotMember)?;
            membership.role = None;
            for peer in membership
                .peers()
                .map(|peer| peer.node_id)
                .collect::<Vec<_>>()
            {
                membership.set_peer_member(peer, false);
            }
            self.store.save(&registry).map_err(|error| {
                CoreError::with_diagnostic(
                    error.kind(),
                    "the joined network was torn down but membership could not be cleared; retry leave",
                    error.to_string(),
                )
            })?;
            // A stale hint is harmless (it is network-scoped and overwritten by the next join),
            // so a failed clear must not turn a completed leave into an error.
            if let Some(records) = &self.client_records {
                let _ = records.clear();
            }
            self.runtime.forget_joined(network);
            Ok(true)
        })
    }
}

impl<R> TransactionalJoinNetworkOperations<R>
where
    R: JoinRuntime,
{
    async fn join_inner(
        &self,
        network: NetworkId,
        runtime_mutated: &mut bool,
    ) -> Result<(), CoreError> {
        {
            // Reject the zero sentinel here, not only in the CLI parser:
            // another D-Bus/API caller must not drive PANU or durable state
            // with an invalid logical network identity.
            if network == NetworkId::from_bytes([0; 16]) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "zero NetworkId is not a joinable BlueRoute network",
                ));
            }
            // Hold this guard through commit and all rollback paths, not just
            // the first preflight; an overlapping join must never undo an
            // earlier attempt's PANU or IP ownership.
            let mut transition = JoinTransitionGuard::claim(&self.join_in_progress)?;
            let registry = self.store.load()?;
            if let Some(current) = current_network(&registry)? {
                return if current == network {
                    Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "durable membership already claims this network; P6-009 runtime reconciliation is required before JoinNetwork can report success",
                    ))
                } else {
                    Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "this daemon already belongs to a different BlueRoute network",
                    ))
                };
            }

            // Derive the exact address before permitting any runtime mutation.
            // Once PANU setup starts, a mismatch must trigger quarantined cleanup.
            let expected_client =
                Ipv4StarAddressPlan::for_network(network, self.expected_ipv4_pool)?.first_client;
            self.runtime.preflight(network).await?;
            // PANU activation may have side effects even when its future
            // fails or is canceled. Never release ownership without cleanup.
            transition.mutation_started();
            *runtime_mutated = true;
            let link = self.runtime.establish_panu(network).await?;
            let lease = match self.runtime.configure_ip(network, &link).await {
                Ok(lease) => lease,
                Err(error) => {
                    // IP setup can partially install an owned address/profile
                    // before returning Err. There is no lease to prove cleanup
                    // even if the PANU disconnect succeeds. Keep joins blocked
                    // until a separate P6-009 reconciliation proves safety.
                    let panu = self.runtime.disconnect_panu(link).await;
                    return Err(combine_rollback(error, Ok(()), Ok(()), panu));
                }
            };
            // A lease attributed to another network or PAN interface is not
            // proof of a BlueRoute-owned address. Never commit membership or
            // ask the runtime to remove that potentially foreign address.
            // The unknown IP mutation requires explicit reconciliation even
            // after disconnecting the known PANU link.
            if lease.address().owner != network || lease.interface() != link.interface() {
                let panu = self.runtime.disconnect_panu(link).await;
                return Err(combine_rollback(
                    CoreError::new(
                        ErrorKind::InvalidState,
                        "PANU join returned an IP lease with mismatched network or interface ownership",
                    ),
                    Ok(()),
                    Ok(()),
                    panu,
                ));
            }
            // The v1 PANU data plane is a private IPv4 star. A same-owner
            // lease for IPv6, a public address, or a loopback address is not
            // evidence that the requested BlueRoute IPv4 segment is ready.
            // Quarantine the unverified address rather than removing a lease
            // whose provenance cannot be trusted after a backend mismatch.
            if !matches!(
                lease.address().prefix.address,
                IpAddr::V4(ip) if ip.is_private() && !ip.is_loopback()
            ) {
                let panu = self.runtime.disconnect_panu(link).await;
                return Err(combine_rollback(
                    CoreError::new(
                        ErrorKind::InvalidState,
                        "PANU join returned an invalid non-private-IPv4 IP lease",
                    ),
                    Ok(()),
                    Ok(()),
                    panu,
                ));
            }
            // A private address from some other star (or a different prefix
            // length within this star) does not establish this network's
            // deterministic PANU lease. Treat it as an unknown backend side
            // effect rather than attempting foreign-address cleanup.
            if lease.address().prefix != expected_client {
                let panu = self.runtime.disconnect_panu(link).await;
                return Err(combine_rollback(
                    CoreError::new(
                        ErrorKind::AddressConflict,
                        "PANU join IP lease differs from deterministic network address plan",
                    ),
                    Ok(()),
                    Ok(()),
                    panu,
                ));
            }
            let session = match self
                .runtime
                .start_control_session(network, &link, &lease)
                .await
            {
                Ok(session) => session,
                Err(error) => {
                    // Control-session setup may have opened a TLS transport
                    // before returning Err without handing back its owner.
                    // Even after IP and PANU cleanup, that unknown socket
                    // cannot be proven closed; deny new joins until P6-009
                    // runtime reconciliation establishes a clean slate.
                    let (error, _) = self.rollback_after_ip(error, lease, link).await;
                    return Err(error);
                }
            };

            // A runtime session authenticated for another network must never
            // authorize durable membership in the requested network. Reject
            // the mismatch before persistence and roll back all owned resources.
            if session.network() != network {
                let error = CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "authenticated control session belongs to a different BlueRoute network",
                );
                let (error, clean) = self
                    .rollback_after_control(error, session, lease, link)
                    .await;
                if clean {
                    transition.reconciled();
                }
                return Err(error);
            }

            let mut registry = match self.store.load() {
                Ok(registry) => registry,
                Err(error) => {
                    let (error, clean) = self
                        .rollback_after_control(error, session, lease, link)
                        .await;
                    if clean {
                        transition.reconciled();
                    }
                    return Err(error);
                }
            };
            match current_network(&registry) {
                Ok(None) => {}
                Ok(Some(_)) => {
                    let error = CoreError::new(
                        ErrorKind::InvalidState,
                        "BlueRoute membership changed while join networking was being established",
                    );
                    let (error, clean) = self
                        .rollback_after_control(error, session, lease, link)
                        .await;
                    if clean {
                        transition.reconciled();
                    }
                    return Err(error);
                }
                Err(error) => {
                    let (error, clean) = self
                        .rollback_after_control(error, session, lease, link)
                        .await;
                    if clean {
                        transition.reconciled();
                    }
                    return Err(error);
                }
            }

            let previous_registry = registry.clone();
            if let Err(error) = mark_local_membership_joined(&mut registry, network) {
                let (error, clean) = self
                    .rollback_after_control(error, session, lease, link)
                    .await;
                if clean {
                    transition.reconciled();
                }
                return Err(error);
            }

            let receipt = match &self.authorization {
                Some(authorization) => match authorization.prepare_commit(network, &mut registry) {
                    Ok(receipt) => Some(receipt),
                    Err(error) => {
                        let (error, clean) = self
                            .rollback_after_control(error, session, lease, link)
                            .await;
                        if clean {
                            transition.reconciled();
                        }
                        return Err(error);
                    }
                },
                None => None,
            };

            if let Err(error) = self.store.save(&registry) {
                let error = match (&self.authorization, receipt) {
                    (Some(authorization), Some(receipt)) => {
                        match authorization.undo_commit(&receipt) {
                            Ok(()) => error,
                            Err(undo) => CoreError::with_diagnostic(
                                error.kind(),
                                error.message(),
                                format!(
                                    "{}; also failed to undo the host key pin: {undo}",
                                    error.diagnostic().unwrap_or("no primary diagnostic")
                                ),
                            ),
                        }
                    }
                    _ => error,
                };
                let (error, clean) = self
                    .rollback_after_control(error, session, lease, link)
                    .await;
                if clean {
                    transition.reconciled();
                }
                return Err(error);
            }

            // Forget the operator's pending host only now that membership is durable. If that
            // fails, revert the whole commit: a stale record must never survive a join.
            if let (Some(authorization), Some(receipt)) = (&self.authorization, &receipt)
                && let Err(finalize_error) = authorization.finalize_commit(receipt)
            {
                let mut diagnostics = Vec::new();
                if let Err(error) = self.store.save(&previous_registry) {
                    diagnostics.push(format!("restoring membership failed: {error}"));
                }
                if let Err(error) = authorization.undo_commit(receipt) {
                    diagnostics.push(format!("undoing the host key pin failed: {error}"));
                }
                let error = if diagnostics.is_empty() {
                    finalize_error
                } else {
                    CoreError::with_diagnostic(
                        finalize_error.kind(),
                        finalize_error.message(),
                        diagnostics.join("; "),
                    )
                };
                let (error, clean) = self
                    .rollback_after_control(error, session, lease, link)
                    .await;
                if clean {
                    transition.reconciled();
                }
                return Err(error);
            }

            // The reconnect hint is advisory (the locator is the fallback and the pinned
            // handshake authenticates the host), so failing to write it must not undo a join
            // whose membership is already durable.
            if let (Some(records), Some(receipt)) = (&self.client_records, &receipt) {
                let _ = ClientRecord::new(network, receipt.host(), Some(link.peer().clone()))
                    .and_then(|record| records.save(Some(&record)));
            }

            // Keep the live link and lease so leave can reverse them; the control session stays
            // owned by the supervisor worker.
            self.runtime.adopt_joined(network, link, lease);
            transition.reconciled();
            Ok(())
        }
    }
}

/// Production join runtime. It stays **closed by construction**: `hardware_verified` is `false`
/// and nothing outside tests can set it, because the supervised control session and two-machine
/// hardware evidence do not exist yet (P6-004). Defense in depth: both `preflight` and
/// `establish_panu` check the flag, so a Bluetooth link is never made while it is off.
#[derive(Clone)]
pub struct LinuxJoinRuntime {
    pool: Ipv4AddressPool,
    locator: Arc<dyn PeerLocator>,
    pending: Option<Arc<PendingJoinOperations>>,
    control: Option<JoinControlConfig>,
    supervisors: Arc<ControlSupervisors>,
    retained: RetainedLinks,
    hardware_verified: bool,
}

impl Default for LinuxJoinRuntime {
    fn default() -> Self {
        Self {
            pool: Ipv4AddressPool::default(),
            locator: Arc::new(BluezPeerLocator),
            pending: None,
            control: None,
            supervisors: Arc::new(ControlSupervisors::new()),
            retained: Arc::default(),
            hardware_verified: false,
        }
    }
}

impl std::fmt::Debug for LinuxJoinRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxJoinRuntime")
            .field("pool", &self.pool)
            .field("hardware_verified", &self.hardware_verified)
            .finish_non_exhaustive()
    }
}

impl LinuxJoinRuntime {
    pub fn new(pool: Ipv4AddressPool) -> Result<Self, CoreError> {
        pool.validate()?;
        Ok(Self {
            pool,
            ..Self::default()
        })
    }

    /// The operator-confirmed host record that must exist before any Bluetooth mutation.
    pub fn with_pending_join(mut self, pending: Arc<PendingJoinOperations>) -> Self {
        self.pending = Some(pending);
        self
    }

    /// TLS identity, trust state and registry for the supervised control session.
    pub fn with_control(mut self, control: JoinControlConfig) -> Self {
        self.control = Some(control);
        self
    }

    /// The workers that own live control sessions, so leave and health reporting can reach them.
    pub fn supervisors(&self) -> Arc<ControlSupervisors> {
        self.supervisors.clone()
    }

    #[cfg(test)]
    fn hardware_verified_for_tests(mut self) -> Self {
        self.hardware_verified = true;
        self
    }

    fn require_open(&self) -> Result<(), CoreError> {
        require_hardware_verified(self.hardware_verified)
    }

    /// The restart-reconciliation runtime. It shares the supervisors and retained links with
    /// this runtime and inherits the same closed gate. `None` without a control configuration.
    pub fn client_runtime(&self) -> Option<SystemClientRuntime> {
        Some(SystemClientRuntime::new(
            self.pool,
            self.locator.clone(),
            self.control.clone()?,
            self.supervisors.clone(),
            self.retained.clone(),
            self.hardware_verified,
        ))
    }
}

/// The single definition of the closed gate, so join and reconnection cannot drift apart.
pub(crate) fn require_hardware_verified(verified: bool) -> Result<(), CoreError> {
    if verified {
        return Ok(());
    }
    Err(CoreError::with_diagnostic(
        ErrorKind::CapabilityUnavailable,
        "BlueRoute join requires P7 authenticated control-session support",
        "P6-005 IPv4 planning and NetworkManager address application are available, but production JoinNetwork still refuses Bluetooth mutation until the supervised authenticated control session exists and two-machine hardware acceptance is recorded",
    ))
}

/// Start the supervised control session on its own worker, shared by join and restart
/// reconciliation so both dial the same pinned way.
pub(crate) async fn start_supervised_control(
    supervisors: &ControlSupervisors,
    control: JoinControlConfig,
    pool: Ipv4AddressPool,
    expected: PinnedControlTlsPeer,
    link: JoinPanLink,
    lease: JoinIpLease,
) -> Result<(), CoreError> {
    supervisors
        .start(expected.network(), move |stop, established| {
            run_join_control(&control, expected, &link, &lease, pool, stop, established)
        })
        .await
}

/// Find the one candidate host and connect PANU to it. If the returned attachment fails
/// validation the just-made link is torn down rather than reported as nothing.
pub(crate) async fn establish_panu_with<Connect, ConnectFuture, Cleanup, CleanupFuture>(
    locator: &dyn PeerLocator,
    network: NetworkId,
    connect: Connect,
    cleanup: Cleanup,
) -> Result<JoinPanLink, CoreError>
where
    Connect: FnOnce(PeerHandle) -> ConnectFuture,
    ConnectFuture: Future<Output = Result<PanAttachment, CoreError>>,
    Cleanup: FnOnce(PanAttachment) -> CleanupFuture,
    CleanupFuture: Future<Output = Result<(), CoreError>>,
{
    let peer = locator.locate(network).await?;
    let attachment = connect(peer.clone()).await?;
    match JoinPanLink::new(peer, attachment.clone()) {
        Ok(link) => Ok(link),
        Err(error) => match cleanup(attachment).await {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(CoreError::with_diagnostic(
                error.kind(),
                error.message(),
                format!("also failed to disconnect the unexpected PANU link: {cleanup_error}"),
            )),
        },
    }
}

impl JoinRuntime for LinuxJoinRuntime {
    fn preflight(&self, network: NetworkId) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async move {
            self.require_open()?;
            // No Bluetooth mutation without an operator-confirmed host for this network.
            let pending = self.pending.as_ref().ok_or_else(|| {
                CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "join has no operator-confirmed host store configured",
                )
            })?;
            pending.host_for(network)?;
            Ok(())
        })
    }

    fn establish_panu(&self, network: NetworkId) -> JoinNetworkFuture<'_, JoinPanLink> {
        Box::pin(async move {
            self.require_open()?;
            let bluez = BluezBackend::connect_system().await?;
            establish_panu_with(
                self.locator.as_ref(),
                network,
                |peer| bluez.connect_panu(peer),
                |attachment| {
                    let bluez = &bluez;
                    async move { bluez.disconnect_matching_panu(&attachment).await }
                },
            )
            .await
        })
    }

    fn disconnect_panu(&self, link: JoinPanLink) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async move {
            let bluez = BluezBackend::connect_system().await?;
            bluez.disconnect_matching_panu(&link.attachment).await
        })
    }

    fn configure_ip<'a>(
        &'a self,
        network: NetworkId,
        link: &'a JoinPanLink,
    ) -> JoinNetworkFuture<'a, JoinIpLease> {
        Box::pin(async move {
            let plan = Ipv4StarAddressPlan::for_network(network, self.pool)?;
            let backend = NetworkManagerBackend::connect_system().await?;
            let active = backend.active_ipv4_prefixes().await?;
            ensure_ipv4_segment_available(plan.segment, active)?;
            let address = InterfaceAddress {
                interface: link.interface().clone(),
                prefix: plan.first_client,
                owner: network,
            };
            backend.ensure_address(address.clone()).await?;
            Ok(JoinIpLease::new(address))
        })
    }

    fn remove_ip(&self, lease: JoinIpLease) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async move {
            let backend = NetworkManagerBackend::connect_system().await?;
            let address = lease.address;
            let remove_address = backend.remove_address(address.clone()).await;
            let remove_profile = backend
                .remove_owned_interface(address.owner, address.interface.clone())
                .await;
            match (remove_address, remove_profile) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(address_error), Ok(())) => Err(address_error),
                (Ok(()), Err(profile_error)) => Err(profile_error),
                (Err(address_error), Err(profile_error)) => Err(CoreError::with_diagnostic(
                    address_error.kind(),
                    address_error.message(),
                    format!(
                        "{}; removing the BlueRoute-owned NetworkManager profile also failed: {profile_error}",
                        address_error
                            .diagnostic()
                            .unwrap_or("no address cleanup diagnostic")
                    ),
                )),
            }
        })
    }

    fn start_control_session<'a>(
        &'a self,
        network: NetworkId,
        link: &'a JoinPanLink,
        lease: &'a JoinIpLease,
    ) -> JoinNetworkFuture<'a, JoinControlSession> {
        Box::pin(async move {
            self.require_open()?;
            let control = self.control.clone().ok_or_else(|| {
                CoreError::new(
                    ErrorKind::CapabilityUnavailable,
                    "join control-session configuration is missing",
                )
            })?;
            let pending = self.pending.as_ref().ok_or_else(|| {
                CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "join has no operator-confirmed host store configured",
                )
            })?;
            // The pin always comes from the operator's record, never from the connection.
            let host = pending.host_for(network)?;
            let expected = PinnedControlTlsPeer::new(network, host.node(), host.fingerprint())
                .map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::AuthenticationFailed,
                        "the confirmed host identity cannot be pinned",
                        error.to_string(),
                    )
                })?;
            start_supervised_control(
                &self.supervisors,
                control,
                self.pool,
                expected,
                link.clone(),
                lease.clone(),
            )
            .await?;
            Ok(JoinControlSession::new(network))
        })
    }

    fn stop_control_session(&self, session: JoinControlSession) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async move { self.supervisors.stop(session.network()) })
    }

    fn adopt_joined(&self, network: NetworkId, link: JoinPanLink, _lease: JoinIpLease) {
        if let Ok(mut retained) = self.retained.lock() {
            retained.insert(network, link.attachment().clone());
        }
    }

    fn leave_joined(
        &self,
        network: NetworkId,
        hint: Option<PeerHandle>,
    ) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async move {
            // The control session first, so nothing keeps using the link while it is removed.
            self.supervisors.stop(network)?;
            let retained = self
                .retained
                .lock()
                .ok()
                .and_then(|retained| retained.get(&network).cloned());
            let backend = SystemClientLeave::connect().await?;
            leave_observed(&backend, network, retained.as_ref(), hint.as_ref()).await
        })
    }

    fn forget_joined(&self, network: NetworkId) {
        if let Ok(mut retained) = self.retained.lock() {
            retained.remove(&network);
        }
    }
}

fn mark_local_membership_joined(
    registry: &mut MembershipRegistry,
    network: NetworkId,
) -> Result<(), CoreError> {
    if let Some(membership) = registry.network_mut(&network) {
        membership.state = membership
            .state
            .transition(MembershipState::Joining)?
            .transition(MembershipState::Member)?;
        membership.role = Some(MembershipRole::Client);
    } else {
        let mut membership = NetworkMembership::new(network, discovered_network_name(network)?);
        membership.state = membership
            .state
            .transition(MembershipState::Joining)?
            .transition(MembershipState::Member)?;
        membership.role = Some(MembershipRole::Client);
        registry.remember_network(membership);
    }
    Ok(())
}

fn discovered_network_name(network: NetworkId) -> Result<DisplayName, CoreError> {
    let id = network.to_string();
    DisplayName::new(format!("BlueRoute {}", &id[..8]))
}

fn combine_rollback(
    primary: CoreError,
    control: Result<(), CoreError>,
    address: Result<(), CoreError>,
    panu: Result<(), CoreError>,
) -> CoreError {
    let failures: Vec<String> = [
        control
            .err()
            .map(|error| format!("control rollback: {error}")),
        address
            .err()
            .map(|error| format!("address rollback: {error}")),
        panu.err().map(|error| format!("PANU rollback: {error}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if failures.is_empty() {
        primary
    } else {
        CoreError::with_diagnostic(
            primary.kind(),
            primary.message(),
            format!(
                "{}; rollback failures: {}",
                primary.diagnostic().unwrap_or("no primary diagnostic"),
                failures.join("; ")
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Step {
        Preflight,
        Panu,
        Address,
        Control,
        StopControl,
        RemoveAddress,
        DisconnectPanu,
    }

    struct FakeRuntime {
        steps: Mutex<Vec<Step>>,
        fail_address: bool,
        fail_control: bool,
        session_network_override: Option<NetworkId>,
        lease_owner_override: Option<NetworkId>,
        lease_interface_override: Option<NetworkInterfaceHandle>,
        lease_prefix_override: Option<blueroute_core::IpPrefix>,
        concurrent_member: Option<(std::path::PathBuf, NetworkId)>,
        pause_preflight: bool,
        pause_rollback_until: Option<std::sync::Arc<AtomicBool>>,
        joined: Mutex<bool>,
        fail_leave_address: AtomicBool,
    }

    impl FakeRuntime {
        fn joined_client(&self, _network: NetworkId) -> bool {
            *self.joined.lock().unwrap()
        }

        fn successful() -> Self {
            Self {
                steps: Mutex::new(Vec::new()),
                fail_address: false,
                fail_control: false,
                session_network_override: None,
                lease_owner_override: None,
                lease_interface_override: None,
                lease_prefix_override: None,
                concurrent_member: None,
                pause_preflight: false,
                pause_rollback_until: None,
                joined: Mutex::new(false),
                fail_leave_address: AtomicBool::new(false),
            }
        }

        fn steps(&self) -> Vec<Step> {
            self.steps.lock().unwrap().clone()
        }

        fn push(&self, step: Step) {
            self.steps.lock().unwrap().push(step);
        }
    }

    impl JoinRuntime for FakeRuntime {
        fn preflight(&self, _network: NetworkId) -> JoinNetworkFuture<'_, ()> {
            Box::pin(async move {
                self.push(Step::Preflight);
                if self.pause_preflight {
                    std::future::pending::<()>().await;
                }
                Ok(())
            })
        }

        fn establish_panu(&self, _network: NetworkId) -> JoinNetworkFuture<'_, JoinPanLink> {
            Box::pin(async move {
                self.push(Step::Panu);
                let peer = PeerHandle::new("/org/bluez/hci0/dev_FA_KE").unwrap();
                JoinPanLink::new(
                    peer.clone(),
                    PanAttachment {
                        role: PanRole::Panu,
                        interface: NetworkInterfaceHandle::new("bnep-test").unwrap(),
                        peer: Some(peer),
                    },
                )
            })
        }

        fn disconnect_panu(&self, _link: JoinPanLink) -> JoinNetworkFuture<'_, ()> {
            Box::pin(async move {
                self.push(Step::DisconnectPanu);
                Ok(())
            })
        }

        fn configure_ip<'a>(
            &'a self,
            _network: NetworkId,
            link: &'a JoinPanLink,
        ) -> JoinNetworkFuture<'a, JoinIpLease> {
            Box::pin(async move {
                self.push(Step::Address);
                if self.fail_address {
                    Err(CoreError::new(
                        ErrorKind::AddressConflict,
                        "injected address failure",
                    ))
                } else {
                    let plan =
                        Ipv4StarAddressPlan::for_network(_network, Ipv4AddressPool::default())
                            .unwrap();
                    Ok(JoinIpLease::new(InterfaceAddress {
                        interface: self
                            .lease_interface_override
                            .clone()
                            .unwrap_or_else(|| link.interface().clone()),
                        prefix: self.lease_prefix_override.unwrap_or(plan.first_client),
                        owner: self.lease_owner_override.unwrap_or(_network),
                    }))
                }
            })
        }

        fn remove_ip(&self, _lease: JoinIpLease) -> JoinNetworkFuture<'_, ()> {
            Box::pin(async move {
                self.push(Step::RemoveAddress);
                if let Some(release) = &self.pause_rollback_until {
                    std::future::poll_fn(|_cx| {
                        if release.load(Ordering::Acquire) {
                            std::task::Poll::Ready(())
                        } else {
                            std::task::Poll::Pending
                        }
                    })
                    .await;
                }
                Ok(())
            })
        }

        fn start_control_session<'a>(
            &'a self,
            network: NetworkId,
            _link: &'a JoinPanLink,
            _lease: &'a JoinIpLease,
        ) -> JoinNetworkFuture<'a, JoinControlSession> {
            Box::pin(async move {
                self.push(Step::Control);
                if self.fail_control {
                    return Err(CoreError::new(
                        ErrorKind::AuthenticationFailed,
                        "injected control authentication failure",
                    ));
                }
                if let Some((path, concurrent_network)) = &self.concurrent_member {
                    let mut membership = NetworkMembership::new(
                        *concurrent_network,
                        DisplayName::new("Concurrent network").unwrap(),
                    );
                    membership.state = MembershipState::Member;
                    let mut registry = MembershipRegistry::default();
                    registry.remember_network(membership);
                    NetworkMembershipStore::new(path.clone()).save(&registry)?;
                }
                Ok(JoinControlSession::new(
                    self.session_network_override.unwrap_or(network),
                ))
            })
        }

        fn stop_control_session(&self, _session: JoinControlSession) -> JoinNetworkFuture<'_, ()> {
            Box::pin(async move {
                self.push(Step::StopControl);
                Ok(())
            })
        }

        fn adopt_joined(&self, _network: NetworkId, _link: JoinPanLink, _lease: JoinIpLease) {
            *self.joined.lock().unwrap() = true;
        }

        fn leave_joined(
            &self,
            _network: NetworkId,
            _hint: Option<PeerHandle>,
        ) -> JoinNetworkFuture<'_, ()> {
            Box::pin(async move {
                self.push(Step::StopControl);
                self.push(Step::RemoveAddress);
                if self.fail_leave_address.load(Ordering::SeqCst) {
                    return Err(CoreError::new(
                        ErrorKind::NetworkBackendUnavailable,
                        "injected address removal failure",
                    ));
                }
                self.push(Step::DisconnectPanu);
                Ok(())
            })
        }

        fn forget_joined(&self, _network: NetworkId) {
            *self.joined.lock().unwrap() = false;
        }
    }

    fn temp_store(label: &str) -> (std::path::PathBuf, NetworkMembershipStore) {
        let root = std::env::temp_dir().join(format!(
            "blueroute-p6-004-{label}-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        (
            root.clone(),
            NetworkMembershipStore::new(root.join("memberships-v1")),
        )
    }

    #[test]
    fn invalid_zero_network_never_uses_runtime_or_persists_membership() {
        let (root, store) = temp_store("zero-network");
        let operations =
            TransactionalJoinNetworkOperations::with_runtime(store, FakeRuntime::successful());
        let error =
            futures_lite::future::block_on(operations.join_network(NetworkId::from_bytes([0; 16])))
                .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert!(error.message().contains("zero NetworkId"));
        assert!(operations.runtime.steps().is_empty());
        assert!(operations.store.load().unwrap().is_empty());
        // A rejected invalid request must not poison a future legitimate
        // transaction or require physical-state reconciliation.
        let valid = NetworkId::from_bytes([0x55; 16]);
        futures_lite::future::block_on(operations.join_network(valid)).unwrap();
        assert_eq!(operations.runtime.steps().len(), 4);
        assert_eq!(
            operations
                .store
                .load()
                .unwrap()
                .network(&valid)
                .unwrap()
                .state,
            MembershipState::Member
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn overlapping_joins_fail_before_runtime_mutation_and_cancel_releases_guard() {
        use std::task::{Context, Waker};

        let (root, store) = temp_store("overlapping-joins");
        let network = NetworkId::from_bytes([0x7a; 16]);
        let other = NetworkId::from_bytes([0x7b; 16]);
        let operations = TransactionalJoinNetworkOperations::with_runtime(
            store,
            FakeRuntime {
                pause_preflight: true,
                ..FakeRuntime::successful()
            },
        );
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        // Drive one join until preflight suspends while holding ownership.
        let mut first = operations.join_network(network);
        assert!(first.as_mut().poll(&mut context).is_pending());
        assert_eq!(operations.runtime.steps(), vec![Step::Preflight]);

        // Both a same-network duplicate and a different-network request
        // must fail before they can allocate IPs, create PANU links or
        // overwrite the first join's membership.
        for attempted in [network, other] {
            let error = futures_lite::future::block_on(operations.join_network(attempted))
                .expect_err("overlapping join must fail closed");
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert!(error.message().contains("already in progress"));
        }
        assert_eq!(operations.runtime.steps(), vec![Step::Preflight]);
        assert!(operations.store.load().unwrap().is_empty());

        // Cancelling a suspended future must not strand the coordinator.
        drop(first);
        let mut restarted = operations.join_network(other);
        assert!(restarted.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            operations.runtime.steps(),
            vec![Step::Preflight, Step::Preflight]
        );
        drop(restarted);
        assert!(operations.store.load().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn competing_join_is_denied_until_reverse_order_rollback_finishes() {
        use std::sync::Arc;
        use std::task::{Context, Poll, Waker};

        let (root, store) = temp_store("pending-rollback");
        let network = NetworkId::from_bytes([0x8a; 16]);
        let competitor = NetworkId::from_bytes([0x8b; 16]);
        let release = Arc::new(AtomicBool::new(false));
        let runtime = FakeRuntime {
            fail_control: true,
            session_network_override: None,
            lease_owner_override: None,
            lease_interface_override: None,
            lease_prefix_override: None,
            pause_rollback_until: Some(Arc::clone(&release)),
            ..FakeRuntime::successful()
        };
        let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);
        let mut first = operations.join_network(network);
        let mut context = Context::from_waker(Waker::noop());

        // Authentication fails only after PANU and IP setup. Block inside
        // reverse-order address cleanup, after the control error is known.
        assert!(first.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            operations.runtime.steps(),
            vec![
                Step::Preflight,
                Step::Panu,
                Step::Address,
                Step::Control,
                Step::RemoveAddress,
            ]
        );
        for attempted in [network, competitor] {
            let error = futures_lite::future::block_on(operations.join_network(attempted))
                .expect_err("another join must not start during teardown");
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert!(error.message().contains("already in progress"));
        }
        assert_eq!(operations.runtime.steps().len(), 5);
        assert!(operations.store.load().unwrap().is_empty());

        // Completing address rollback must then disconnect the PANU and
        // release the transaction guard, preserving the original error.
        release.store(true, Ordering::Release);
        match first.as_mut().poll(&mut context) {
            Poll::Ready(Err(error)) => assert_eq!(error.kind(), ErrorKind::AuthenticationFailed),
            other => panic!("rollback did not finish: {other:?}"),
        }
        drop(first);
        assert_eq!(
            operations.runtime.steps().last(),
            Some(&Step::DisconnectPanu)
        );
        // The first failed control setup did not return a session handle:
        // successful IP/PANU teardown does not prove a half-open TLS socket
        // was closed, so no new preflight is permitted yet.
        let new_error = futures_lite::future::block_on(operations.join_network(competitor))
            .expect_err("ambiguous control setup requires reconciliation");
        assert_eq!(new_error.kind(), ErrorKind::InvalidState);
        assert!(new_error.message().contains("reconciliation"));
        assert_eq!(
            operations.runtime.steps().last(),
            Some(&Step::DisconnectPanu)
        );
        assert_eq!(operations.runtime.steps().len(), 6);
        assert!(operations.store.load().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canceled_join_during_rollback_blocks_reuse_until_reconciliation() {
        use std::sync::Arc;
        use std::task::{Context, Waker};

        let (root, store) = temp_store("canceled-rollback");
        let network = NetworkId::from_bytes([0x8c; 16]);
        let competitor = NetworkId::from_bytes([0x8d; 16]);
        let release = Arc::new(AtomicBool::new(false));
        let runtime = FakeRuntime {
            fail_control: true,
            session_network_override: None,
            lease_owner_override: None,
            lease_interface_override: None,
            lease_prefix_override: None,
            pause_rollback_until: Some(Arc::clone(&release)),
            ..FakeRuntime::successful()
        };
        let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);
        let mut first = operations.join_network(network);
        let mut context = Context::from_waker(Waker::noop());

        assert!(first.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            operations.runtime.steps().last(),
            Some(&Step::RemoveAddress)
        );
        // Dropping the future aborts pending cleanup; opening the gate
        // cannot resume a canceled rollback.
        drop(first);
        release.store(true, Ordering::Release);
        for attempted in [network, competitor] {
            let error = futures_lite::future::block_on(operations.join_network(attempted))
                .expect_err("canceled cleanup must poison the join coordinator");
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert!(error.message().contains("reconciliation"));
        }
        assert_eq!(
            operations.runtime.steps(),
            vec![
                Step::Preflight,
                Step::Panu,
                Step::Address,
                Step::Control,
                Step::RemoveAddress,
            ]
        );
        assert!(operations.store.load().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn membership_is_committed_only_after_all_runtime_steps_succeed() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("success");
            let network = NetworkId::from_bytes([0x44; 16]);
            let runtime = FakeRuntime::successful();
            let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);

            operations.join_network(network).await.unwrap();
            assert_eq!(
                operations.runtime.steps(),
                vec![Step::Preflight, Step::Panu, Step::Address, Step::Control]
            );
            let registry = operations.store.load().unwrap();
            assert_eq!(
                registry.network(&network).unwrap().state,
                MembershipState::Member
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn address_failure_disconnects_panu_and_does_not_persist_membership() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("address-failure");
            let network = NetworkId::from_bytes([0x45; 16]);
            let runtime = FakeRuntime {
                steps: Mutex::new(Vec::new()),
                fail_address: true,
                fail_control: false,
                session_network_override: None,
                lease_owner_override: None,
                lease_interface_override: None,
                lease_prefix_override: None,
                concurrent_member: None,
                pause_preflight: false,
                pause_rollback_until: None,
                joined: Mutex::new(false),
                fail_leave_address: AtomicBool::new(false),
            };
            let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);

            let error = operations.join_network(network).await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::AddressConflict);
            assert_eq!(
                operations.runtime.steps(),
                vec![
                    Step::Preflight,
                    Step::Panu,
                    Step::Address,
                    Step::DisconnectPanu
                ]
            );
            assert!(operations.store.load().unwrap().is_empty());
            // The failed backend might have installed IP state without
            // returning a lease. PANU-only teardown cannot prove recovery.
            let denied = operations.join_network(network).await.unwrap_err();
            assert_eq!(denied.kind(), ErrorKind::InvalidState);
            assert!(denied.message().contains("reconciliation"));
            assert_eq!(
                operations.runtime.steps(),
                vec![
                    Step::Preflight,
                    Step::Panu,
                    Step::Address,
                    Step::DisconnectPanu,
                ]
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn foreign_ip_lease_never_authorizes_membership_or_foreign_cleanup() {
        futures_lite::future::block_on(async {
            for scenario in 0..2 {
                let (root, store) = temp_store(&format!("lease-mismatch-{scenario}"));
                let network = NetworkId::from_bytes([0x69; 16]);
                let mut runtime = FakeRuntime::successful();
                if scenario == 0 {
                    runtime.lease_owner_override = Some(NetworkId::from_bytes([0x70; 16]));
                } else {
                    runtime.lease_interface_override =
                        Some(NetworkInterfaceHandle::new("foreign-ip").unwrap());
                }
                let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);

                let error = operations.join_network(network).await.unwrap_err();
                assert_eq!(error.kind(), ErrorKind::InvalidState);
                assert_eq!(
                    operations.runtime.steps(),
                    vec![
                        Step::Preflight,
                        Step::Panu,
                        Step::Address,
                        Step::DisconnectPanu,
                    ]
                );
                assert!(operations.store.load().unwrap().is_empty());
                // Removing this unverified lease could mutate a foreign owner.
                // Even successful PANU teardown does not prove the address
                // safe, so another join requires explicit reconciliation.
                let error = operations.join_network(network).await.unwrap_err();
                assert_eq!(error.kind(), ErrorKind::InvalidState);
                assert!(error.message().contains("reconciliation"));
                assert_eq!(operations.runtime.steps().len(), 4);
                fs::remove_dir_all(root).unwrap();
            }
        });
    }

    #[test]
    fn nonprivate_or_non_ipv4_lease_never_enables_join_or_foreign_cleanup() {
        futures_lite::future::block_on(async {
            for (label, address, prefix_len) in [
                ("public", "8.8.8.8", 24),
                ("loopback", "127.0.0.1", 8),
                ("ipv6", "2001:db8::2", 64),
            ] {
                let (root, store) = temp_store(label);
                let network = NetworkId::from_bytes([0x69; 16]);
                let mut runtime = FakeRuntime::successful();
                runtime.lease_prefix_override = Some(
                    blueroute_core::IpPrefix::new(address.parse().unwrap(), prefix_len).unwrap(),
                );
                let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);
                let error = operations.join_network(network).await.unwrap_err();
                assert_eq!(error.kind(), ErrorKind::InvalidState);
                assert_eq!(
                    operations.runtime.steps(),
                    vec![
                        Step::Preflight,
                        Step::Panu,
                        Step::Address,
                        Step::DisconnectPanu
                    ]
                );
                assert!(operations.store.load().unwrap().is_empty());
                let retry = operations.join_network(network).await.unwrap_err();
                assert_eq!(retry.kind(), ErrorKind::InvalidState);
                assert!(retry.message().contains("reconciliation"));
                assert_eq!(operations.runtime.steps().len(), 4);
                fs::remove_dir_all(root).unwrap();
            }
        });
    }

    #[test]
    fn wrong_private_star_address_or_prefix_never_commits_membership() {
        futures_lite::future::block_on(async {
            let network = NetworkId::from_bytes([0x69; 16]);
            let planned = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default())
                .unwrap()
                .first_client;
            let different_private =
                blueroute_core::IpPrefix::new("192.168.43.2".parse().unwrap(), planned.prefix_len)
                    .unwrap();
            let wrong_prefix =
                blueroute_core::IpPrefix::new(planned.address, planned.prefix_len + 1).unwrap();
            for (label, unexpected) in [
                ("foreign-star", different_private),
                ("wrong-prefix", wrong_prefix),
            ] {
                assert_ne!(unexpected, planned);
                let (root, store) = temp_store(label);
                let mut runtime = FakeRuntime::successful();
                runtime.lease_prefix_override = Some(unexpected);
                let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);
                let error = operations.join_network(network).await.unwrap_err();
                assert_eq!(error.kind(), ErrorKind::AddressConflict);
                assert_eq!(
                    operations.runtime.steps(),
                    vec![
                        Step::Preflight,
                        Step::Panu,
                        Step::Address,
                        Step::DisconnectPanu,
                    ]
                );
                assert!(operations.store.load().unwrap().is_empty());
                let retry = operations.join_network(network).await.unwrap_err();
                assert_eq!(retry.kind(), ErrorKind::InvalidState);
                assert!(retry.message().contains("reconciliation"));
                fs::remove_dir_all(root).unwrap();
            }
        });
    }

    #[test]
    fn control_failure_removes_ip_then_disconnects_panu() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("control-failure");
            let network = NetworkId::from_bytes([0x46; 16]);
            let runtime = FakeRuntime {
                steps: Mutex::new(Vec::new()),
                fail_address: false,
                fail_control: true,
                session_network_override: None,
                lease_owner_override: None,
                lease_interface_override: None,
                lease_prefix_override: None,
                concurrent_member: None,
                pause_preflight: false,
                pause_rollback_until: None,
                joined: Mutex::new(false),
                fail_leave_address: AtomicBool::new(false),
            };
            let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);

            let error = operations.join_network(network).await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
            assert_eq!(
                operations.runtime.steps(),
                vec![
                    Step::Preflight,
                    Step::Panu,
                    Step::Address,
                    Step::Control,
                    Step::RemoveAddress,
                    Step::DisconnectPanu,
                ]
            );
            assert!(operations.store.load().unwrap().is_empty());
            let denied = operations.join_network(network).await.unwrap_err();
            assert_eq!(denied.kind(), ErrorKind::InvalidState);
            assert!(denied.message().contains("reconciliation"));
            assert_eq!(operations.runtime.steps().len(), 6);
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn mismatched_control_network_rolls_back_without_committing_membership() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("wrong-control-network");
            let network = NetworkId::from_bytes([0x46; 16]);
            let mut runtime = FakeRuntime::successful();
            runtime.session_network_override = Some(NetworkId::from_bytes([0x77; 16]));
            let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);

            let error = operations.join_network(network).await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
            assert_eq!(
                operations.runtime.steps(),
                vec![
                    Step::Preflight,
                    Step::Panu,
                    Step::Address,
                    Step::Control,
                    Step::StopControl,
                    Step::RemoveAddress,
                    Step::DisconnectPanu,
                ]
            );
            assert!(operations.store.load().unwrap().is_empty());
            // A complete rollback releases the guard for a later attempt,
            // but it cannot convert a wrong-network session into success.
            operations.runtime.steps.lock().unwrap().clear();
            let repeated_error = operations.join_network(network).await.unwrap_err();
            assert_eq!(repeated_error.kind(), ErrorKind::AuthenticationFailed);
            assert!(operations.store.load().unwrap().is_empty());
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn the_join_gate_opens_only_in_a_feature_build_with_everything_wired() {
        let (root, store) = temp_store("gate-open");
        let pool = Ipv4AddressPool::default();
        let pending = Arc::new(PendingJoinOperations::new(
            NetworkMembershipStore::new(root.join("pj-memberships")),
            crate::PendingJoinStore::new(root.join("pj")),
        ));
        let node = blueroute_core::NodeId::from_bytes([0x49; 16]);
        let control = || JoinControlConfig {
            local: Arc::new(crate::LocalControlTlsIdentity::generate(node).unwrap()),
            trust: Arc::new(crate::DurablePeerTrustOperations::new(
                NetworkMembershipStore::new(root.join("trust-memberships")),
            )),
            registry: Arc::new(Mutex::new(
                blueroute_protocol::session::ControlSessionRegistry::new(),
            )),
            capabilities: blueroute_core::NodeCapabilities::default(),
        };
        let bare = || {
            TransactionalJoinNetworkOperations::with_ipv4_pool(
                NetworkMembershipStore::new(store.path().to_path_buf()),
                pool,
                pending.clone(),
                control(),
            )
            .unwrap()
        };
        let authorization = || -> Arc<dyn JoinCommitAuthorization> {
            Arc::new(crate::PendingHostCommitAuthorization::new(
                pending.clone(),
                crate::PeerPinStore::new(root.join("pins")),
            ))
        };
        let records = || Arc::new(ClientRecordStore::new(root.join("client-record")));
        let refused = |result: Result<TransactionalJoinNetworkOperations, CoreError>| {
            assert_eq!(
                result.err().unwrap().kind(),
                ErrorKind::CapabilityUnavailable
            );
        };

        // A build without the feature can only refuse, even fully wired: this is what CI and
        // the package build.
        let wired = || {
            bare()
                .with_authorization(authorization())
                .with_client_records(records())
        };
        refused(wired().open_join_gate(false));
        if !EXPERIMENTAL_JOIN {
            refused(wired().open_join_gate_for_hardware_testing());
        }
        // With the feature, every dependency must be present.
        refused(bare().with_client_records(records()).open_join_gate(true));
        refused(
            bare()
                .with_authorization(authorization())
                .open_join_gate(true),
        );
        let without_control = TransactionalJoinNetworkOperations::with_runtime_and_pool(
            NetworkMembershipStore::new(store.path().to_path_buf()),
            LinuxJoinRuntime::new(pool).unwrap(),
            pool,
        )
        .with_authorization(authorization())
        .with_client_records(records());
        refused(without_control.open_join_gate(true));

        let open = wired().open_join_gate(true).unwrap();
        assert!(open.runtime.hardware_verified);
        // Restart reconnection shares the same opened gate.
        assert!(open.client_runtime().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    /// Guards against the feature ever becoming a default: CI and the Debian package build
    /// without features, so this must hold there.
    #[cfg(not(feature = "experimental-join"))]
    #[test]
    fn default_builds_cannot_open_the_join_gate() {
        const { assert!(!EXPERIMENTAL_JOIN) };
        let manifest = include_str!("../Cargo.toml");
        let features = manifest
            .split_once("[features]")
            .map_or("", |(_, rest)| rest);
        // Only a real `default = [...]` key inside the [features] table counts; comments do not.
        let declares_default = features
            .lines()
            .take_while(|line| !line.trim_start().starts_with('['))
            .any(|line| line.trim_start().starts_with("default"));
        assert!(!declares_default, "no default features may be declared");
    }

    #[test]
    fn configured_ipv4_pool_is_retained_by_production_join_coordinator() {
        let (root, store) = temp_store("custom-pool");
        let pool = Ipv4AddressPool::new("192.168.200.0".parse().unwrap(), 24, 24).unwrap();
        let pending = Arc::new(PendingJoinOperations::new(
            NetworkMembershipStore::new(root.join("pj-memberships")),
            crate::PendingJoinStore::new(root.join("pj")),
        ));
        let node = blueroute_core::NodeId::from_bytes([0x48; 16]);
        let control = JoinControlConfig {
            local: Arc::new(crate::LocalControlTlsIdentity::generate(node).unwrap()),
            trust: Arc::new(crate::DurablePeerTrustOperations::new(
                NetworkMembershipStore::new(root.join("trust-memberships")),
            )),
            registry: Arc::new(Mutex::new(
                blueroute_protocol::session::ControlSessionRegistry::new(),
            )),
            capabilities: blueroute_core::NodeCapabilities::default(),
        };
        let operations =
            TransactionalJoinNetworkOperations::with_ipv4_pool(store, pool, pending, control)
                .unwrap();
        assert_eq!(operations.expected_ipv4_pool, pool);
        let error = futures_lite::future::block_on(
            operations.join_network(NetworkId::from_bytes([0x47; 16])),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::CapabilityUnavailable);
        assert!(operations.store.load().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_preflight_blocks_before_unsafe_partial_join() {
        let (root, store) = temp_store("blocked");
        let operations = TransactionalJoinNetworkOperations::new(store);
        let error = futures_lite::future::block_on(
            operations.join_network(NetworkId::from_bytes([0x47; 16])),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::CapabilityUnavailable);
        assert!(error.message().contains("requires P7"));
        assert!(operations.store.load().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_durable_membership_change_rolls_everything_back() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("concurrent-membership");
            let network = NetworkId::from_bytes([0x49; 16]);
            let concurrent_network = NetworkId::from_bytes([0x59; 16]);
            let runtime = FakeRuntime {
                steps: Mutex::new(Vec::new()),
                fail_address: false,
                fail_control: false,
                session_network_override: None,
                lease_owner_override: None,
                lease_interface_override: None,
                lease_prefix_override: None,
                concurrent_member: Some((store.path().to_owned(), concurrent_network)),
                pause_preflight: false,
                pause_rollback_until: None,
                joined: Mutex::new(false),
                fail_leave_address: AtomicBool::new(false),
            };
            let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);

            let error = operations.join_network(network).await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert_eq!(
                operations.runtime.steps(),
                vec![
                    Step::Preflight,
                    Step::Panu,
                    Step::Address,
                    Step::Control,
                    Step::StopControl,
                    Step::RemoveAddress,
                    Step::DisconnectPanu,
                ]
            );
            let registry = operations.store.load().unwrap();
            assert!(registry.network(&network).is_none());
            assert_eq!(
                registry.network(&concurrent_network).unwrap().state,
                MembershipState::Member
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn rollback_failures_are_never_silently_discarded() {
        let error = combine_rollback(
            CoreError::new(ErrorKind::AuthenticationFailed, "primary failure"),
            Err(CoreError::new(
                ErrorKind::ProtocolError,
                "control cleanup failed",
            )),
            Ok(()),
            Err(CoreError::new(ErrorKind::PanFailure, "PANU cleanup failed")),
        );
        assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
        assert_eq!(error.message(), "primary failure");
        let diagnostic = error.diagnostic().unwrap();
        assert!(diagnostic.contains("control rollback: control cleanup failed"));
        assert!(diagnostic.contains("PANU rollback: PANU cleanup failed"));
    }

    #[test]
    fn durable_member_without_runtime_does_not_fake_join_success() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("idempotent");
            let network = NetworkId::from_bytes([0x48; 16]);
            let mut membership =
                NetworkMembership::new(network, DisplayName::new("Already joined").unwrap());
            membership.state = MembershipState::Member;
            let mut registry = MembershipRegistry::default();
            registry.remember_network(membership);
            store.save(&registry).unwrap();
            let runtime = FakeRuntime::successful();
            let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime);

            let error = operations.join_network(network).await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert!(error.message().contains("P6-009"));
            assert!(operations.runtime.steps().is_empty());
            fs::remove_dir_all(root).unwrap();
        });
    }

    mod authorized_commit {
        use blueroute_protocol::peer_pin::SpkiSha256;

        use super::*;
        use crate::{
            PeerPinStore, PendingHostCommitAuthorization, PendingJoinHost, PendingJoinOperations,
            PendingJoinStore,
        };

        fn network() -> NetworkId {
            NetworkId::from_bytes([0x71; 16])
        }

        fn host() -> blueroute_core::NodeId {
            blueroute_core::NodeId::from_bytes([0x72; 16])
        }

        fn key() -> SpkiSha256 {
            SpkiSha256::from_verified_certificate_digest([0x73; 32]).unwrap()
        }

        fn operations(
            label: &str,
            runtime: FakeRuntime,
        ) -> (
            std::path::PathBuf,
            TransactionalJoinNetworkOperations<FakeRuntime>,
            std::sync::Arc<PendingJoinOperations>,
        ) {
            let (root, store) = temp_store(label);
            let pending = std::sync::Arc::new(PendingJoinOperations::new(
                NetworkMembershipStore::new(root.join("memberships-v1")),
                PendingJoinStore::new(root.join("pending")),
            ));
            let operations = TransactionalJoinNetworkOperations::with_runtime(store, runtime)
                .with_authorization(std::sync::Arc::new(PendingHostCommitAuthorization::new(
                    pending.clone(),
                    PeerPinStore::new(root.join("pins")),
                )));
            (root, operations, pending)
        }

        fn record(pending: &PendingJoinOperations) {
            pending
                .record_host(PendingJoinHost::new(network(), host(), key()).unwrap())
                .unwrap();
        }

        #[test]
        fn success_commits_member_with_trusted_pinned_host_and_clears_the_record() {
            let (root, operations, pending) = operations("auth-ok", FakeRuntime::successful());
            record(&pending);
            futures_lite::future::block_on(operations.join_network(network())).unwrap();
            let registry = NetworkMembershipStore::new(root.join("memberships-v1"))
                .load()
                .unwrap();
            let membership = registry.network(&network()).unwrap();
            assert_eq!(membership.state, MembershipState::Member);
            assert_eq!(membership.role, Some(MembershipRole::Client));
            assert!(membership.is_peer_trusted(&host()));
            assert!(
                PeerPinStore::new(root.join("pins"))
                    .load()
                    .unwrap()
                    .authorize(network(), host(), true, key())
                    .is_ok()
            );
            assert_eq!(pending.pending_host().unwrap(), None);
            let _ = fs::remove_dir_all(root);
        }

        fn with_records(
            label: &str,
            runtime: FakeRuntime,
        ) -> (
            std::path::PathBuf,
            TransactionalJoinNetworkOperations<FakeRuntime>,
            std::sync::Arc<PendingJoinOperations>,
            std::sync::Arc<crate::ClientRecordStore>,
        ) {
            let (root, operations, pending) = operations(label, runtime);
            let records = std::sync::Arc::new(crate::ClientRecordStore::new(root.join("client")));
            (
                root,
                operations.with_client_records(records.clone()),
                pending,
                records,
            )
        }

        #[test]
        fn a_committed_join_records_the_host_and_the_device_hint() {
            let (root, operations, pending, records) =
                with_records("hint-write", FakeRuntime::successful());
            record(&pending);
            futures_lite::future::block_on(operations.join_network(network())).unwrap();
            let stored = records.load_for(network()).unwrap().unwrap();
            assert_eq!(stored.host(), host());
            assert!(stored.device_hint().is_some());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_failed_join_leaves_no_client_record() {
            let (root, operations, pending, records) =
                with_records("hint-failed", FakeRuntime::successful());
            // No pending host recorded, so the commit is refused.
            let _ = futures_lite::future::block_on(operations.join_network(network())).unwrap_err();
            assert_eq!(records.load().unwrap(), None);
            drop(pending);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn leave_clears_the_record_and_a_failed_leave_keeps_it() {
            let (root, operations, pending, records) =
                with_records("hint-leave", FakeRuntime::successful());
            record(&pending);
            futures_lite::future::block_on(operations.join_network(network())).unwrap();
            operations
                .runtime
                .fail_leave_address
                .store(true, Ordering::SeqCst);
            futures_lite::future::block_on(operations.leave_joined()).unwrap_err();
            assert!(records.load_for(network()).unwrap().is_some());
            operations
                .runtime
                .fail_leave_address
                .store(false, Ordering::SeqCst);
            assert!(futures_lite::future::block_on(operations.leave_joined()).unwrap());
            assert_eq!(records.load().unwrap(), None);
            let _ = fs::remove_dir_all(root);
        }

        struct FailingFinalize(PendingHostCommitAuthorization);

        impl crate::JoinCommitAuthorization for FailingFinalize {
            fn prepare_commit(
                &self,
                network: NetworkId,
                registry: &mut MembershipRegistry,
            ) -> Result<crate::JoinCommitReceipt, CoreError> {
                self.0.prepare_commit(network, registry)
            }

            fn finalize_commit(
                &self,
                _receipt: &crate::JoinCommitReceipt,
            ) -> Result<(), CoreError> {
                Err(CoreError::new(
                    ErrorKind::PersistenceError,
                    "injected failure clearing the pending host",
                ))
            }

            fn undo_commit(&self, receipt: &crate::JoinCommitReceipt) -> Result<(), CoreError> {
                self.0.undo_commit(receipt)
            }

            fn discard(&self, network: NetworkId) -> Result<(), CoreError> {
                self.0.discard(network)
            }
        }

        #[test]
        fn a_failed_finalize_reverts_membership_and_pin_so_no_stale_record_survives_a_join() {
            let (root, store) = temp_store("auth-finalize");
            let pending = std::sync::Arc::new(PendingJoinOperations::new(
                NetworkMembershipStore::new(root.join("memberships-v1")),
                PendingJoinStore::new(root.join("pending")),
            ));
            let operations =
                TransactionalJoinNetworkOperations::with_runtime(store, FakeRuntime::successful())
                    .with_authorization(std::sync::Arc::new(FailingFinalize(
                        PendingHostCommitAuthorization::new(
                            pending.clone(),
                            PeerPinStore::new(root.join("pins")),
                        ),
                    )));
            record(&pending);
            let error =
                futures_lite::future::block_on(operations.join_network(network())).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::PersistenceError);
            let registry = NetworkMembershipStore::new(root.join("memberships-v1"))
                .load()
                .unwrap();
            assert!(registry.network(&network()).is_none());
            assert!(
                PeerPinStore::new(root.join("pins"))
                    .load()
                    .unwrap()
                    .is_empty()
            );
            assert!(operations.runtime.steps().ends_with(&[
                Step::StopControl,
                Step::RemoveAddress,
                Step::DisconnectPanu,
            ]));
            // The wrapper then forgets the operator's intent after the failed attempt.
            assert_eq!(pending.pending_host().unwrap(), None);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_missing_host_record_rolls_back_everything_and_commits_no_membership() {
            let (root, operations, _pending) = operations("auth-none", FakeRuntime::successful());
            let error =
                futures_lite::future::block_on(operations.join_network(network())).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
            assert_eq!(
                operations.runtime.steps(),
                vec![
                    Step::Preflight,
                    Step::Panu,
                    Step::Address,
                    Step::Control,
                    Step::StopControl,
                    Step::RemoveAddress,
                    Step::DisconnectPanu,
                ]
            );
            let registry = NetworkMembershipStore::new(root.join("memberships-v1"))
                .load()
                .unwrap();
            assert!(registry.network(&network()).is_none());
            assert!(
                PeerPinStore::new(root.join("pins"))
                    .load()
                    .unwrap()
                    .is_empty()
            );
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_failed_attempt_after_runtime_mutation_forgets_the_pending_host() {
            let mut runtime = FakeRuntime::successful();
            runtime.fail_control = true;
            let (root, operations, pending) = operations("auth-fail", runtime);
            record(&pending);
            assert!(futures_lite::future::block_on(operations.join_network(network())).is_err());
            assert_eq!(pending.pending_host().unwrap(), None);
            assert!(
                PeerPinStore::new(root.join("pins"))
                    .load()
                    .unwrap()
                    .is_empty()
            );
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_refusal_before_any_runtime_mutation_keeps_the_pending_host() {
            let (root, operations, pending) = operations("auth-early", FakeRuntime::successful());
            record(&pending);
            // Becomes a member of a different network after the host was recorded.
            let store = NetworkMembershipStore::new(root.join("memberships-v1"));
            let mut other = NetworkMembership::new(
                NetworkId::from_bytes([0x74; 16]),
                DisplayName::new("Other").unwrap(),
            );
            other.state = MembershipState::Member;
            let mut registry = MembershipRegistry::default();
            registry.remember_network(other);
            store.save(&registry).unwrap();
            assert!(futures_lite::future::block_on(operations.join_network(network())).is_err());
            assert!(operations.runtime.steps().is_empty());
            assert!(
                PendingJoinStore::new(root.join("pending"))
                    .load()
                    .unwrap()
                    .is_some()
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    mod production_runtime {
        use std::sync::atomic::AtomicUsize;

        use blueroute_protocol::peer_pin::SpkiSha256;

        use super::*;
        use crate::{PendingJoinHost, PendingJoinStore};

        struct FakeLocator(Result<PeerHandle, CoreError>);

        impl PeerLocator for FakeLocator {
            fn locate(&self, _network: NetworkId) -> JoinNetworkFuture<'_, PeerHandle> {
                let result = self.0.clone();
                Box::pin(async move { result })
            }
        }

        fn net() -> NetworkId {
            NetworkId::from_bytes([0x81; 16])
        }

        fn peer(name: &str) -> PeerHandle {
            PeerHandle::new(format!("/org/bluez/hci0/dev_{name}")).unwrap()
        }

        fn attachment(for_peer: Option<PeerHandle>, role: PanRole) -> PanAttachment {
            PanAttachment {
                role,
                interface: NetworkInterfaceHandle::new("bnep-test").unwrap(),
                peer: for_peer,
            }
        }

        fn pending(label: &str) -> (std::path::PathBuf, Arc<PendingJoinOperations>) {
            let (root, _) = temp_store(label);
            let pending = Arc::new(PendingJoinOperations::new(
                NetworkMembershipStore::new(root.join("memberships-v1")),
                PendingJoinStore::new(root.join("pending")),
            ));
            (root, pending)
        }

        #[test]
        fn the_production_runtime_is_closed_by_default_and_never_reaches_bluetooth() {
            let runtime = LinuxJoinRuntime::default();
            let preflight = futures_lite::future::block_on(runtime.preflight(net())).unwrap_err();
            assert_eq!(preflight.kind(), ErrorKind::CapabilityUnavailable);
            // Closed before any BlueZ connection: this would otherwise fail differently (no bus).
            let panu = futures_lite::future::block_on(runtime.establish_panu(net())).unwrap_err();
            assert_eq!(panu.kind(), ErrorKind::CapabilityUnavailable);
            // The control session is closed too, and nothing was registered by the attempt.
            let link = JoinPanLink::new(
                peer("AA_00_00_00_00_01"),
                attachment(Some(peer("AA_00_00_00_00_01")), PanRole::Panu),
            )
            .unwrap();
            let lease = JoinIpLease::new(InterfaceAddress {
                interface: link.interface().clone(),
                prefix: Ipv4StarAddressPlan::for_network(net(), Ipv4AddressPool::default())
                    .unwrap()
                    .first_client,
                owner: net(),
            });
            let control =
                futures_lite::future::block_on(runtime.start_control_session(net(), &link, &lease))
                    .unwrap_err();
            assert_eq!(control.kind(), ErrorKind::CapabilityUnavailable);
            assert_eq!(runtime.supervisors().state(net()), None);
            // Stopping a network with no worker is a harmless no-op.
            futures_lite::future::block_on(
                runtime.stop_control_session(JoinControlSession::new(net())),
            )
            .unwrap();
        }

        #[test]
        fn even_when_open_preflight_needs_the_confirmed_host() {
            let (root, pending) = pending("rt-preflight");
            let without_store = LinuxJoinRuntime::default().hardware_verified_for_tests();
            assert_eq!(
                futures_lite::future::block_on(without_store.preflight(net()))
                    .unwrap_err()
                    .kind(),
                ErrorKind::AuthenticationFailed
            );
            let runtime = LinuxJoinRuntime::default()
                .hardware_verified_for_tests()
                .with_pending_join(pending.clone());
            assert_eq!(
                futures_lite::future::block_on(runtime.preflight(net()))
                    .unwrap_err()
                    .kind(),
                ErrorKind::AuthenticationFailed
            );
            let key = SpkiSha256::from_verified_certificate_digest([9; 32]).unwrap();
            pending
                .record_host(
                    PendingJoinHost::new(net(), blueroute_core::NodeId::from_bytes([3; 16]), key)
                        .unwrap(),
                )
                .unwrap();
            futures_lite::future::block_on(runtime.preflight(net())).unwrap();
            // A host recorded for another network is not enough.
            assert!(
                futures_lite::future::block_on(
                    runtime.preflight(NetworkId::from_bytes([0x82; 16]))
                )
                .is_err()
            );
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn locator_failure_means_no_connection_attempt() {
            let connects = AtomicUsize::new(0);
            let locator = FakeLocator(Err(CoreError::new(
                ErrorKind::InvalidState,
                "several nearby devices",
            )));
            let error = futures_lite::future::block_on(establish_panu_with(
                &locator,
                net(),
                |_| async {
                    connects.fetch_add(1, Ordering::SeqCst);
                    Err(CoreError::new(ErrorKind::Internal, "must not connect"))
                },
                |_| async { Ok(()) },
            ))
            .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert_eq!(connects.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn the_located_peer_is_connected_and_returned() {
            let host = peer("AA_00_00_00_00_01");
            let locator = FakeLocator(Ok(host.clone()));
            let link = futures_lite::future::block_on(establish_panu_with(
                &locator,
                net(),
                |peer| async move { Ok(attachment(Some(peer), PanRole::Panu)) },
                |_| async { Err(CoreError::new(ErrorKind::Internal, "no cleanup expected")) },
            ))
            .unwrap();
            assert_eq!(link.peer(), &host);
        }

        #[test]
        fn an_attachment_for_a_different_peer_or_role_is_torn_down_not_adopted() {
            let host = peer("AA_00_00_00_00_01");
            for bad in [
                attachment(Some(peer("BB_00_00_00_00_09")), PanRole::Panu),
                attachment(Some(host.clone()), PanRole::Nap),
                attachment(None, PanRole::Panu),
            ] {
                let cleaned = AtomicUsize::new(0);
                let locator = FakeLocator(Ok(host.clone()));
                let bad_for_connect = bad.clone();
                let error = futures_lite::future::block_on(establish_panu_with(
                    &locator,
                    net(),
                    |_| async move { Ok(bad_for_connect) },
                    |seen| {
                        assert_eq!(seen, bad);
                        cleaned.fetch_add(1, Ordering::SeqCst);
                        async { Ok(()) }
                    },
                ))
                .unwrap_err();
                assert_eq!(error.kind(), ErrorKind::InvalidState);
                assert_eq!(cleaned.load(Ordering::SeqCst), 1);
            }
        }

        #[test]
        fn a_failed_teardown_of_an_unexpected_link_is_reported() {
            let host = peer("AA_00_00_00_00_01");
            let locator = FakeLocator(Ok(host.clone()));
            let error = futures_lite::future::block_on(establish_panu_with(
                &locator,
                net(),
                |_| async { Ok(attachment(Some(peer("BB_00_00_00_00_09")), PanRole::Panu)) },
                |_| async { Err(CoreError::new(ErrorKind::PanFailure, "disconnect refused")) },
            ))
            .unwrap_err();
            assert!(error.diagnostic().unwrap().contains("disconnect refused"));
        }
    }

    mod joined_leave {
        use super::*;

        fn net() -> NetworkId {
            NetworkId::from_bytes([0x91; 16])
        }

        fn joined(
            label: &str,
        ) -> (
            std::path::PathBuf,
            TransactionalJoinNetworkOperations<FakeRuntime>,
        ) {
            let (root, store) = temp_store(label);
            let operations =
                TransactionalJoinNetworkOperations::with_runtime(store, FakeRuntime::successful());
            futures_lite::future::block_on(operations.join_network(net())).unwrap();
            operations.runtime.steps.lock().unwrap().clear();
            (root, operations)
        }

        fn state(root: &std::path::Path) -> Option<MembershipState> {
            NetworkMembershipStore::new(root.join("memberships-v1"))
                .load()
                .unwrap()
                .network(&net())
                .map(|membership| membership.state)
        }

        #[test]
        fn leaving_reverses_the_join_in_order_and_clears_membership() {
            let (root, operations) = joined("leave-ok");
            assert_eq!(state(&root), Some(MembershipState::Member));
            assert!(futures_lite::future::block_on(operations.leave_joined()).unwrap());
            assert_eq!(
                operations.runtime.steps(),
                vec![Step::StopControl, Step::RemoveAddress, Step::DisconnectPanu]
            );
            assert_eq!(state(&root), Some(MembershipState::NotMember));
            assert!(!operations.runtime.joined_client(net()));
            // Nothing left to leave: a second call is a clean "not a joined client".
            assert!(!futures_lite::future::block_on(operations.leave_joined()).unwrap());
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_teardown_failure_keeps_member_and_can_be_retried() {
            let (root, operations) = joined("leave-retry");
            operations
                .runtime
                .fail_leave_address
                .store(true, Ordering::SeqCst);
            let error = futures_lite::future::block_on(operations.leave_joined()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::NetworkBackendUnavailable);
            assert_eq!(state(&root), Some(MembershipState::Member));
            assert!(operations.runtime.joined_client(net()));
            operations
                .runtime
                .fail_leave_address
                .store(false, Ordering::SeqCst);
            assert!(futures_lite::future::block_on(operations.leave_joined()).unwrap());
            assert_eq!(state(&root), Some(MembershipState::NotMember));
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_node_that_is_not_a_joined_client_is_left_to_host_leave() {
            // No network at all.
            let (root, store) = temp_store("leave-none");
            let operations =
                TransactionalJoinNetworkOperations::with_runtime(store, FakeRuntime::successful());
            assert!(!futures_lite::future::block_on(operations.leave_joined()).unwrap());
            assert!(operations.runtime.steps().is_empty());
            // A member with no retained client state (for example a host, or after a restart).
            let mut membership = NetworkMembership::new(net(), DisplayName::new("Hosted").unwrap());
            membership.state = MembershipState::Member;
            let mut registry = MembershipRegistry::default();
            registry.remember_network(membership);
            operations.store.save(&registry).unwrap();
            assert!(!futures_lite::future::block_on(operations.leave_joined()).unwrap());
            assert!(operations.runtime.steps().is_empty());
            assert_eq!(state(&root), Some(MembershipState::Member));
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn a_restarted_client_with_a_persisted_client_role_can_leave_without_retained_state() {
            let (root, store) = temp_store("leave-restarted");
            let operations =
                TransactionalJoinNetworkOperations::with_runtime(store, FakeRuntime::successful());
            let mut membership = NetworkMembership::new(net(), DisplayName::new("Joined").unwrap());
            membership.state = MembershipState::Member;
            membership.role = Some(MembershipRole::Client);
            let mut registry = MembershipRegistry::default();
            registry.remember_network(membership);
            operations.store.save(&registry).unwrap();
            // A fresh runtime: nothing was adopted, as after a daemon restart.
            assert!(!operations.runtime.joined_client(net()));
            assert!(futures_lite::future::block_on(operations.leave_joined()).unwrap());
            assert_eq!(
                operations.runtime.steps(),
                [Step::StopControl, Step::RemoveAddress, Step::DisconnectPanu]
            );
            assert_eq!(state(&root), Some(MembershipState::NotMember));
            let membership = operations.store.load().unwrap();
            assert_eq!(membership.network(&net()).unwrap().role, None);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn leave_is_refused_while_a_join_is_in_progress() {
            let (root, store) = temp_store("leave-busy");
            let operations =
                TransactionalJoinNetworkOperations::with_runtime(store, FakeRuntime::successful());
            let _busy = JoinTransitionGuard::claim(&operations.join_in_progress).unwrap();
            let error = futures_lite::future::block_on(operations.leave_joined()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            let _ = fs::remove_dir_all(root);
        }
    }
}

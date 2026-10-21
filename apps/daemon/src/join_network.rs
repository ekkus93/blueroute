use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use blueroute_core::{
    CoreError, DisplayName, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, MembershipRegistry,
    MembershipState, NetworkId, NetworkMembership, ensure_ipv4_segment_available,
};
use blueroute_linux::{
    InterfaceAddress, IpNetworkBackend, IpNetworkObservationBackend, NetworkInterfaceHandle,
    NetworkManagerBackend, NetworkMembershipStore, NetworkStateBackend, PanAttachment, PanRole,
    PeerHandle,
};

use crate::current_network;

pub type JoinNetworkFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

pub trait JoinNetworkOperations: Send + Sync {
    fn join_network(&self, network: NetworkId) -> JoinNetworkFuture<'_, ()>;
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
    join_in_progress: AtomicBool,
}

impl TransactionalJoinNetworkOperations<LinuxJoinRuntime> {
    pub fn new(store: NetworkMembershipStore) -> Self {
        Self::with_runtime(store, LinuxJoinRuntime::default())
    }

    pub fn with_ipv4_pool(
        store: NetworkMembershipStore,
        pool: Ipv4AddressPool,
    ) -> Result<Self, CoreError> {
        Ok(Self::with_runtime(store, LinuxJoinRuntime::new(pool)?))
    }
}

impl<R> TransactionalJoinNetworkOperations<R>
where
    R: JoinRuntime,
{
    pub fn with_runtime(store: NetworkMembershipStore, runtime: R) -> Self {
        Self {
            store,
            runtime,
            join_in_progress: AtomicBool::new(false),
        }
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

            self.runtime.preflight(network).await?;
            // PANU activation may have side effects even when its future
            // fails or is canceled. Never release ownership without cleanup.
            transition.mutation_started();
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

            if let Err(error) = mark_local_membership_joined(&mut registry, network) {
                let (error, clean) = self
                    .rollback_after_control(error, session, lease, link)
                    .await;
                if clean {
                    transition.reconciled();
                }
                return Err(error);
            }

            if let Err(error) = self.store.save(&registry) {
                let (error, clean) = self
                    .rollback_after_control(error, session, lease, link)
                    .await;
                if clean {
                    transition.reconciled();
                }
                return Err(error);
            }

            transition.reconciled();
            Ok(())
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxJoinRuntime {
    pool: Ipv4AddressPool,
}

impl LinuxJoinRuntime {
    pub fn new(pool: Ipv4AddressPool) -> Result<Self, CoreError> {
        pool.validate()?;
        Ok(Self { pool })
    }
}

impl JoinRuntime for LinuxJoinRuntime {
    fn preflight(&self, _network: NetworkId) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::with_diagnostic(
                ErrorKind::CapabilityUnavailable,
                "BlueRoute join requires P7 authenticated control-session support",
                "P6-005 IPv4 planning and NetworkManager address application are available, but production JoinNetwork still refuses Bluetooth mutation until authenticated control bootstrap is implemented",
            ))
        })
    }

    fn establish_panu(&self, _network: NetworkId) -> JoinNetworkFuture<'_, JoinPanLink> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "production PANU join activation is blocked until authenticated control bootstrap is complete",
            ))
        })
    }

    fn disconnect_panu(&self, _link: JoinPanLink) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "production PANU cleanup is not implemented for JoinNetwork yet",
            ))
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
        _network: NetworkId,
        _link: &'a JoinPanLink,
        _lease: &'a JoinIpLease,
    ) -> JoinNetworkFuture<'a, JoinControlSession> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "P7 authenticated control-session support is required before PANU join can commit membership",
            ))
        })
    }

    fn stop_control_session(&self, _session: JoinControlSession) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "production control-session cleanup is not implemented for JoinNetwork yet",
            ))
        })
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
    } else {
        let mut membership = NetworkMembership::new(network, discovered_network_name(network)?);
        membership.state = membership
            .state
            .transition(MembershipState::Joining)?
            .transition(MembershipState::Member)?;
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
    }

    impl FakeRuntime {
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
                        prefix: self
                            .lease_prefix_override
                            .clone()
                            .unwrap_or(plan.first_client),
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
}

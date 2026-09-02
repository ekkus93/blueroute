use std::future::Future;
use std::pin::Pin;

use blueroute_core::{
    CoreError, DisplayName, ErrorKind, MembershipRegistry, MembershipState, NetworkId,
    NetworkMembership,
};
use blueroute_linux::{
    NetworkInterfaceHandle, NetworkMembershipStore, PanAttachment, PanRole, PeerHandle,
};

use crate::current_network;

pub type JoinNetworkFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

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
    interface: NetworkInterfaceHandle,
}

impl JoinIpLease {
    pub fn new(interface: NetworkInterfaceHandle) -> Self {
        Self { interface }
    }

    pub fn interface(&self) -> &NetworkInterfaceHandle {
        &self.interface
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

pub struct TransactionalJoinNetworkOperations<R = LinuxJoinRuntime> {
    store: NetworkMembershipStore,
    runtime: R,
}

impl TransactionalJoinNetworkOperations<LinuxJoinRuntime> {
    pub fn new(store: NetworkMembershipStore) -> Self {
        Self::with_runtime(store, LinuxJoinRuntime)
    }
}

impl<R> TransactionalJoinNetworkOperations<R>
where
    R: JoinRuntime,
{
    pub fn with_runtime(store: NetworkMembershipStore, runtime: R) -> Self {
        Self { store, runtime }
    }

    async fn rollback_after_control(
        &self,
        primary: CoreError,
        session: JoinControlSession,
        lease: JoinIpLease,
        link: JoinPanLink,
    ) -> CoreError {
        let control = self.runtime.stop_control_session(session).await;
        let address = self.runtime.remove_ip(lease).await;
        let panu = self.runtime.disconnect_panu(link).await;
        combine_rollback(primary, control, address, panu)
    }

    async fn rollback_after_ip(
        &self,
        primary: CoreError,
        lease: JoinIpLease,
        link: JoinPanLink,
    ) -> CoreError {
        let address = self.runtime.remove_ip(lease).await;
        let panu = self.runtime.disconnect_panu(link).await;
        combine_rollback(primary, Ok(()), address, panu)
    }
}

impl<R> JoinNetworkOperations for TransactionalJoinNetworkOperations<R>
where
    R: JoinRuntime,
{
    fn join_network(&self, network: NetworkId) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async move {
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
            let link = self.runtime.establish_panu(network).await?;
            let lease = match self.runtime.configure_ip(network, &link).await {
                Ok(lease) => lease,
                Err(error) => {
                    let panu = self.runtime.disconnect_panu(link).await;
                    return Err(combine_rollback(error, Ok(()), Ok(()), panu));
                }
            };
            let session = match self
                .runtime
                .start_control_session(network, &link, &lease)
                .await
            {
                Ok(session) => session,
                Err(error) => return Err(self.rollback_after_ip(error, lease, link).await),
            };

            let mut registry = match self.store.load() {
                Ok(registry) => registry,
                Err(error) => {
                    return Err(self
                        .rollback_after_control(error, session, lease, link)
                        .await);
                }
            };
            match current_network(&registry) {
                Ok(None) => {}
                Ok(Some(_)) => {
                    let error = CoreError::new(
                        ErrorKind::InvalidState,
                        "BlueRoute membership changed while join networking was being established",
                    );
                    return Err(self
                        .rollback_after_control(error, session, lease, link)
                        .await);
                }
                Err(error) => {
                    return Err(self
                        .rollback_after_control(error, session, lease, link)
                        .await);
                }
            }

            if let Err(error) = mark_local_membership_joined(&mut registry, network) {
                return Err(self
                    .rollback_after_control(error, session, lease, link)
                    .await);
            }

            if let Err(error) = self.store.save(&registry) {
                return Err(self
                    .rollback_after_control(error, session, lease, link)
                    .await);
            }

            Ok(())
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxJoinRuntime;

impl JoinRuntime for LinuxJoinRuntime {
    fn preflight(&self, _network: NetworkId) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::with_diagnostic(
                ErrorKind::CapabilityUnavailable,
                "BlueRoute join requires P6-005 address allocation and P7 authenticated control-session support",
                "production JoinNetwork is refusing to mutate Bluetooth or durable membership state until both prerequisites are implemented",
            ))
        })
    }

    fn establish_panu(&self, _network: NetworkId) -> JoinNetworkFuture<'_, JoinPanLink> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "production PANU join activation is blocked until join prerequisites are complete",
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
        _network: NetworkId,
        _link: &'a JoinPanLink,
    ) -> JoinNetworkFuture<'a, JoinIpLease> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "P6-005 address allocation is required before PANU join can configure IP",
            ))
        })
    }

    fn remove_ip(&self, _lease: JoinIpLease) -> JoinNetworkFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "production PANU address cleanup is not implemented for JoinNetwork yet",
            ))
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
        control.err().map(|error| format!("control rollback: {error}")),
        address.err().map(|error| format!("address rollback: {error}")),
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
    }

    impl FakeRuntime {
        fn successful() -> Self {
            Self {
                steps: Mutex::new(Vec::new()),
                fail_address: false,
                fail_control: false,
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
                    Ok(JoinIpLease::new(link.interface().clone()))
                }
            })
        }

        fn remove_ip(&self, _lease: JoinIpLease) -> JoinNetworkFuture<'_, ()> {
            Box::pin(async move {
                self.push(Step::RemoveAddress);
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
                    Err(CoreError::new(
                        ErrorKind::AuthenticationFailed,
                        "injected control authentication failure",
                    ))
                } else {
                    Ok(JoinControlSession::new(network))
                }
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
            fs::remove_dir_all(root).unwrap();
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
        assert!(error.message().contains("requires P6-005"));
        assert!(operations.store.load().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_durable_state_after_runtime_setup_rolls_everything_back() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("invalid-durable-state");
            let network = NetworkId::from_bytes([0x49; 16]);
            let mut membership = NetworkMembership::new(
                network,
                DisplayName::new("Interrupted leave").unwrap(),
            );
            membership.state = MembershipState::Leaving;
            let mut registry = MembershipRegistry::default();
            registry.remember_network(membership);
            store.save(&registry).unwrap();
            let runtime = FakeRuntime::successful();
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
            assert_eq!(
                registry.network(&network).unwrap().state,
                MembershipState::Leaving
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
            Err(CoreError::new(
                ErrorKind::PanFailure,
                "PANU cleanup failed",
            )),
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
            let mut membership = NetworkMembership::new(
                network,
                DisplayName::new("Already joined").unwrap(),
            );
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

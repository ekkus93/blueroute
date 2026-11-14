//! Observation-based cleanup for a joined client (P6-009 S3, design D4).
//!
//! Leave must work after a daemon restart, when no in-memory handles exist. So it re-derives what
//! to remove from BlueRoute **ownership metadata** instead of remembering it, and it only ever
//! touches state it can prove is this network's:
//!
//! * NetworkManager addresses and profiles that carry this `NetworkId` as owner are removed.
//! * The PANU link is disconnected only when the live BlueZ attachment is for the recorded host
//!   device **and** is proven ours: either it equals the exact attachment this process retained,
//!   or it runs on an interface that carries this network's ownership. Anything else is foreign
//!   and is left alone.
//!
//! The PANU link goes first, while the owned profile still proves it is ours. Every step is
//! idempotent, so a failed leave can be retried and resumes correctly.

use std::collections::BTreeSet;

use blueroute_core::{CoreError, ErrorKind, NetworkId};
use blueroute_linux::{
    BackendFuture, BluezBackend, InterfaceAddress, IpNetworkBackend, NetworkInterfaceHandle,
    NetworkManagerBackend, NetworkStateBackend, PanAttachment, PanRole, PeerHandle,
};

/// What the cleanup needs from the system. Fakeable so every case runs without hardware.
pub trait ClientLeaveBackend: Send + Sync {
    /// Addresses carrying this network's ownership.
    fn owned_addresses(&self, network: NetworkId) -> BackendFuture<'_, Vec<InterfaceAddress>>;
    /// Interfaces of NetworkManager profiles carrying this network's ownership.
    fn owned_profile_interfaces(
        &self,
        network: NetworkId,
    ) -> BackendFuture<'_, Vec<NetworkInterfaceHandle>>;
    fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()>;
    fn remove_owned_interface(
        &self,
        network: NetworkId,
        interface: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, ()>;
    fn observe_panu(&self, peer: PeerHandle) -> BackendFuture<'_, Option<PanAttachment>>;
    /// Whether the host device is present, so that "no link observed" is conclusive.
    fn panu_device_present(&self, peer: PeerHandle) -> BackendFuture<'_, bool>;
    /// Disconnect only if the live attachment still equals `expected`.
    fn disconnect_panu(&self, expected: PanAttachment) -> BackendFuture<'_, ()>;
}

/// Tear down a joined client's PANU link and owned IP configuration for `network`.
///
/// `retained` is the exact link this process created, when it still has it; `hint` is the
/// recorded host device path. Either names the host device to look at; neither is trusted alone.
pub async fn leave_observed(
    backend: &dyn ClientLeaveBackend,
    network: NetworkId,
    retained: Option<&PanAttachment>,
    hint: Option<&PeerHandle>,
) -> Result<(), CoreError> {
    let addresses: Vec<InterfaceAddress> = backend
        .owned_addresses(network)
        .await?
        .into_iter()
        .filter(|address| address.owner == network)
        .collect();
    let mut owned: BTreeSet<NetworkInterfaceHandle> = backend
        .owned_profile_interfaces(network)
        .await?
        .into_iter()
        .collect();
    owned.extend(addresses.iter().map(|address| address.interface.clone()));

    let host = retained
        .and_then(|link| link.peer.clone())
        .or_else(|| hint.cloned());
    if let Some(host) = host
        && let Some(live) = backend.observe_panu(host.clone()).await?
        && live.role == PanRole::Panu
        && live.peer.as_ref() == Some(&host)
        && (retained.is_some_and(|expected| &live == expected) || owned.contains(&live.interface))
    {
        backend.disconnect_panu(live.clone()).await?;
        // Keep the ownership proof until the link is observed gone (FU-04, REMOVE-2): a
        // disconnect that did not take leaves everything in place for a retry.
        if backend.observe_panu(host).await?.as_ref() == Some(&live) {
            return Err(CoreError::new(
                ErrorKind::PanFailure,
                "the BlueRoute PANU link is still connected after disconnecting it",
            ));
        }
    }

    for address in addresses {
        backend.remove_address(address).await?;
    }
    for interface in owned {
        backend.remove_owned_interface(network, interface).await?;
    }
    Ok(())
}

/// Remove every NetworkManager address and profile that carries this network's ownership, and
/// nothing else. No Bluetooth link is looked at. Host leave uses this after a daemon restart, when
/// no runtime handles exist (P6-008): BlueZ drops NAP registrations and advertisements together
/// with the D-Bus connection that made them, so only owned NetworkManager state can remain.
pub async fn remove_owned_network_state(
    backend: &dyn ClientLeaveBackend,
    network: NetworkId,
) -> Result<(), CoreError> {
    leave_observed(backend, network, None, None).await
}

/// The production backend: NetworkManager ownership metadata and, for client leave, BlueZ.
pub struct SystemClientLeave {
    network_manager: NetworkManagerBackend,
    bluez: Option<BluezBackend>,
}

impl SystemClientLeave {
    pub async fn connect() -> Result<Self, CoreError> {
        Ok(Self {
            network_manager: NetworkManagerBackend::connect_system().await?,
            bluez: Some(BluezBackend::connect_system().await?),
        })
    }

    /// NetworkManager only, for `remove_owned_network_state`, so it works while `bluetoothd` is
    /// down. Any PANU request on it is refused.
    pub async fn connect_network_manager() -> Result<Self, CoreError> {
        Ok(Self {
            network_manager: NetworkManagerBackend::connect_system().await?,
            bluez: None,
        })
    }

    fn bluez(&self) -> Result<&BluezBackend, CoreError> {
        self.bluez.as_ref().ok_or_else(|| {
            CoreError::new(
                ErrorKind::Internal,
                "this cleanup backend was opened without Bluetooth access",
            )
        })
    }
}

impl crate::OwnedInventory for SystemClientLeave {
    fn owned_resources(&self) -> BackendFuture<'_, Vec<(NetworkId, crate::OwnedKind)>> {
        Box::pin(async move {
            let mut owned: Vec<(NetworkId, crate::OwnedKind)> = self
                .network_manager
                .addresses()
                .await?
                .into_iter()
                .map(|address| (address.owner, crate::OwnedKind::Address))
                .collect();
            owned.extend(
                self.network_manager
                    .network_connections()
                    .await?
                    .into_iter()
                    .filter_map(|connection| connection.owner)
                    .map(|owner| (owner, crate::OwnedKind::Profile)),
            );
            Ok(owned)
        })
    }
}

impl ClientLeaveBackend for SystemClientLeave {
    fn owned_addresses(&self, network: NetworkId) -> BackendFuture<'_, Vec<InterfaceAddress>> {
        Box::pin(async move {
            Ok(self
                .network_manager
                .addresses()
                .await?
                .into_iter()
                .filter(|address| address.owner == network)
                .collect())
        })
    }

    fn owned_profile_interfaces(
        &self,
        network: NetworkId,
    ) -> BackendFuture<'_, Vec<NetworkInterfaceHandle>> {
        Box::pin(async move {
            Ok(self
                .network_manager
                .network_connections()
                .await?
                .into_iter()
                .filter(|connection| connection.owner == Some(network))
                .filter_map(|connection| connection.interface)
                .collect())
        })
    }

    fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
        self.network_manager.remove_address(address)
    }

    fn remove_owned_interface(
        &self,
        network: NetworkId,
        interface: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, ()> {
        self.network_manager
            .remove_owned_interface(network, interface)
    }

    fn observe_panu(&self, peer: PeerHandle) -> BackendFuture<'_, Option<PanAttachment>> {
        Box::pin(async move { self.bluez()?.observe_panu(&peer).await })
    }

    fn panu_device_present(&self, peer: PeerHandle) -> BackendFuture<'_, bool> {
        Box::pin(async move { self.bluez()?.panu_device_present(&peer).await })
    }

    fn disconnect_panu(&self, expected: PanAttachment) -> BackendFuture<'_, ()> {
        Box::pin(async move { self.bluez()?.disconnect_matching_panu(&expected).await })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use blueroute_core::{CoreError, ErrorKind, IpPrefix};

    use super::*;

    fn net() -> NetworkId {
        NetworkId::from_bytes([0x41; 16])
    }

    fn other() -> NetworkId {
        NetworkId::from_bytes([0x42; 16])
    }

    fn iface(name: &str) -> NetworkInterfaceHandle {
        NetworkInterfaceHandle::new(name).unwrap()
    }

    fn host() -> PeerHandle {
        PeerHandle::new("/org/bluez/hci0/dev_AA_00_00_00_00_01").unwrap()
    }

    fn address(owner: NetworkId, name: &str) -> InterfaceAddress {
        InterfaceAddress {
            interface: iface(name),
            prefix: IpPrefix::new("192.168.77.2".parse().unwrap(), 24).unwrap(),
            owner,
        }
    }

    fn attachment(name: &str, role: PanRole, peer: Option<PeerHandle>) -> PanAttachment {
        PanAttachment {
            role,
            interface: iface(name),
            peer,
        }
    }

    #[derive(Default)]
    struct State {
        addresses: Vec<InterfaceAddress>,
        profiles: Vec<(NetworkId, NetworkInterfaceHandle)>,
        panu: Option<PanAttachment>,
        calls: Vec<String>,
        fail_address_removal: bool,
    }

    #[derive(Default)]
    struct Fake(Mutex<State>);

    impl Fake {
        fn with(build: impl FnOnce(&mut State)) -> Self {
            let fake = Self::default();
            build(&mut fake.0.lock().unwrap());
            fake
        }

        fn calls(&self) -> Vec<String> {
            self.0.lock().unwrap().calls.clone()
        }
    }

    impl ClientLeaveBackend for Fake {
        fn owned_addresses(&self, _network: NetworkId) -> BackendFuture<'_, Vec<InterfaceAddress>> {
            // Deliberately unfiltered: the code under test must not trust the backend's filter.
            Box::pin(async move { Ok(self.0.lock().unwrap().addresses.clone()) })
        }

        fn owned_profile_interfaces(
            &self,
            network: NetworkId,
        ) -> BackendFuture<'_, Vec<NetworkInterfaceHandle>> {
            Box::pin(async move {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .profiles
                    .iter()
                    .filter(|(owner, _)| *owner == network)
                    .map(|(_, interface)| interface.clone())
                    .collect())
            })
        }

        fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                let mut state = self.0.lock().unwrap();
                if state.fail_address_removal {
                    return Err(CoreError::new(
                        ErrorKind::NetworkBackendUnavailable,
                        "injected address removal failure",
                    ));
                }
                state
                    .calls
                    .push(format!("remove_address {}", address.interface.as_str()));
                state.addresses.retain(|existing| existing != &address);
                Ok(())
            })
        }

        fn remove_owned_interface(
            &self,
            network: NetworkId,
            interface: NetworkInterfaceHandle,
        ) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                let mut state = self.0.lock().unwrap();
                state
                    .calls
                    .push(format!("remove_profile {}", interface.as_str()));
                state
                    .profiles
                    .retain(|(owner, existing)| !(*owner == network && existing == &interface));
                Ok(())
            })
        }

        fn panu_device_present(&self, _peer: PeerHandle) -> BackendFuture<'_, bool> {
            Box::pin(async { Ok(true) })
        }

        fn observe_panu(&self, _peer: PeerHandle) -> BackendFuture<'_, Option<PanAttachment>> {
            Box::pin(async move { Ok(self.0.lock().unwrap().panu.clone()) })
        }

        fn disconnect_panu(&self, expected: PanAttachment) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                let mut state = self.0.lock().unwrap();
                state
                    .calls
                    .push(format!("disconnect {}", expected.interface.as_str()));
                state.panu = None;
                Ok(())
            })
        }
    }

    fn run(
        fake: &Fake,
        retained: Option<&PanAttachment>,
        hint: Option<&PeerHandle>,
    ) -> Result<(), CoreError> {
        futures_lite::future::block_on(leave_observed(fake, net(), retained, hint))
    }

    #[test]
    fn nothing_present_is_a_clean_no_op() {
        let fake = Fake::default();
        run(&fake, None, Some(&host())).unwrap();
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn after_a_restart_the_hint_and_ownership_find_and_remove_everything_link_first() {
        let fake = Fake::with(|state| {
            state.addresses.push(address(net(), "bnep0"));
            state.profiles.push((net(), iface("bnep0")));
            state.panu = Some(attachment("bnep0", PanRole::Panu, Some(host())));
        });
        run(&fake, None, Some(&host())).unwrap();
        assert_eq!(
            fake.calls(),
            [
                "disconnect bnep0",
                "remove_address bnep0",
                "remove_profile bnep0"
            ]
        );
    }

    #[test]
    fn a_link_not_on_an_owned_interface_is_foreign_and_left_alone() {
        let fake = Fake::with(|state| {
            state.addresses.push(address(net(), "bnep0"));
            state.panu = Some(attachment("bnep9", PanRole::Panu, Some(host())));
        });
        run(&fake, None, Some(&host())).unwrap();
        assert_eq!(
            fake.calls(),
            ["remove_address bnep0", "remove_profile bnep0"]
        );
        assert!(fake.0.lock().unwrap().panu.is_some());
    }

    #[test]
    fn a_link_to_another_device_or_with_another_role_is_never_disconnected() {
        for live in [
            attachment(
                "bnep0",
                PanRole::Panu,
                Some(PeerHandle::new("/org/bluez/hci0/dev_BB").unwrap()),
            ),
            attachment("bnep0", PanRole::Nap, Some(host())),
            attachment("bnep0", PanRole::Panu, None),
        ] {
            let fake = Fake::with(|state| {
                state.addresses.push(address(net(), "bnep0"));
                state.panu = Some(live);
            });
            run(&fake, None, Some(&host())).unwrap();
            assert_eq!(
                fake.calls(),
                ["remove_address bnep0", "remove_profile bnep0"]
            );
        }
    }

    #[test]
    fn the_exact_retained_attachment_proves_ownership_without_any_profile() {
        let link = attachment("bnep0", PanRole::Panu, Some(host()));
        let fake = Fake::with(|state| state.panu = Some(link.clone()));
        run(&fake, Some(&link), None).unwrap();
        assert_eq!(fake.calls(), ["disconnect bnep0"]);
    }

    #[test]
    fn a_replacement_link_without_ownership_is_not_disconnected() {
        let retained = attachment("bnep0", PanRole::Panu, Some(host()));
        let replacement = attachment("bnep5", PanRole::Panu, Some(host()));
        let fake = Fake::with(|state| state.panu = Some(replacement));
        run(&fake, Some(&retained), None).unwrap();
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn foreign_addresses_and_profiles_are_never_touched() {
        let fake = Fake::with(|state| {
            state.addresses.push(address(other(), "bnep3"));
            state.profiles.push((other(), iface("bnep3")));
            state.addresses.push(address(net(), "bnep0"));
        });
        run(&fake, None, None).unwrap();
        assert_eq!(
            fake.calls(),
            ["remove_address bnep0", "remove_profile bnep0"]
        );
        let state = fake.0.lock().unwrap();
        assert_eq!(state.addresses, [address(other(), "bnep3")]);
        assert_eq!(state.profiles, [(other(), iface("bnep3"))]);
    }

    #[test]
    fn an_owned_profile_without_a_link_or_address_is_still_removed() {
        let fake = Fake::with(|state| state.profiles.push((net(), iface("bnep0"))));
        run(&fake, None, Some(&host())).unwrap();
        assert_eq!(fake.calls(), ["remove_profile bnep0"]);
    }

    #[test]
    fn a_failure_midway_is_retryable_and_the_second_attempt_finishes_the_job() {
        let fake = Fake::with(|state| {
            state.addresses.push(address(net(), "bnep0"));
            state.profiles.push((net(), iface("bnep0")));
            state.panu = Some(attachment("bnep0", PanRole::Panu, Some(host())));
            state.fail_address_removal = true;
        });
        let error = run(&fake, None, Some(&host())).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NetworkBackendUnavailable);
        // The link was already disconnected while ownership was still provable.
        assert_eq!(fake.calls(), ["disconnect bnep0"]);
        fake.0.lock().unwrap().fail_address_removal = false;
        run(&fake, None, Some(&host())).unwrap();
        let state = fake.0.lock().unwrap();
        assert!(state.addresses.is_empty() && state.profiles.is_empty() && state.panu.is_none());
    }

    #[test]
    fn host_cleanup_after_a_restart_removes_only_owned_state_and_never_touches_bluetooth() {
        let fake = Fake::with(|state| {
            state.addresses = vec![address(net(), "brbr0"), address(other(), "brother")];
            state.profiles = vec![(net(), iface("brbr0")), (other(), iface("brother"))];
            // A live PANU link exists, but host cleanup never looks at Bluetooth.
            state.panu = Some(attachment("bnep0", PanRole::Panu, Some(host())));
        });
        futures_lite::future::block_on(remove_owned_network_state(&fake, net())).unwrap();
        assert_eq!(
            fake.calls(),
            ["remove_address brbr0", "remove_profile brbr0"]
        );
        let state = fake.0.lock().unwrap();
        assert_eq!(state.addresses, vec![address(other(), "brother")]);
        assert_eq!(state.profiles, vec![(other(), iface("brother"))]);
        assert!(state.panu.is_some());
        drop(state);
        // Idempotent: a second run (a retried leave) finds nothing of ours.
        futures_lite::future::block_on(remove_owned_network_state(&fake, net())).unwrap();
        assert_eq!(fake.calls().len(), 2);
    }
}

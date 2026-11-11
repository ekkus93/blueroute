//! The production [`ClientRuntime`] for restart reconciliation (P6-009 S5).
//!
//! It observes through BlueZ and NetworkManager ownership metadata and reuses the join path's
//! building blocks (peer locator, deterministic lease plan, supervised pinned control session),
//! so a reconnect dials exactly as a join does. Every mutating call is behind the same closed
//! gate as join: while `hardware_verified` is false nothing here touches Bluetooth or addresses.

use std::sync::Arc;

use blueroute_core::{
    CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId,
    ensure_ipv4_segment_available,
};
use blueroute_linux::{
    BackendFuture, BluezBackend, InterfaceAddress, IpNetworkBackend, IpNetworkObservationBackend,
    NetworkManagerBackend, NetworkStateBackend, PanBackend, PeerHandle,
};

use crate::join_network::{
    RetainedLinks, establish_panu_with, require_hardware_verified, start_supervised_control,
};
use crate::{
    ClientHost, ClientRuntime, ControlSupervisors, JoinControlConfig, JoinIpLease,
    JoinNetworkFuture, JoinPanLink, ObservedClient, PeerLocator, PinnedControlTlsPeer,
};

/// A "locator" that names the recorded device. It is a hint: connecting to it proves nothing,
/// the pinned handshake does.
struct HintLocator(PeerHandle);

impl PeerLocator for HintLocator {
    fn locate(&self, _network: NetworkId) -> JoinNetworkFuture<'_, PeerHandle> {
        let hint = self.0.clone();
        Box::pin(async move { Ok(hint) })
    }
}

pub struct SystemClientRuntime {
    pool: Ipv4AddressPool,
    locator: Arc<dyn PeerLocator>,
    control: JoinControlConfig,
    supervisors: Arc<ControlSupervisors>,
    retained: RetainedLinks,
    hardware_verified: bool,
}

impl SystemClientRuntime {
    pub(crate) fn new(
        pool: Ipv4AddressPool,
        locator: Arc<dyn PeerLocator>,
        control: JoinControlConfig,
        supervisors: Arc<ControlSupervisors>,
        retained: RetainedLinks,
        hardware_verified: bool,
    ) -> Self {
        Self {
            pool,
            locator,
            control,
            supervisors,
            retained,
            hardware_verified,
        }
    }

    fn require_open(&self) -> Result<(), CoreError> {
        require_hardware_verified(self.hardware_verified)
    }
}

impl ClientRuntime for SystemClientRuntime {
    fn observe(
        &self,
        network: NetworkId,
        hint: Option<PeerHandle>,
    ) -> BackendFuture<'_, ObservedClient> {
        Box::pin(async move {
            let network_manager = NetworkManagerBackend::connect_system().await?;
            let owned_addresses: Vec<InterfaceAddress> = network_manager
                .addresses()
                .await?
                .into_iter()
                .filter(|address| address.owner == network)
                .collect();
            let owned_profile = network_manager
                .network_connections()
                .await?
                .iter()
                .any(|connection| connection.owner == Some(network));
            // Without a recorded device there is nothing to look at; the locator takes over.
            let panu = match hint {
                Some(peer) => {
                    BluezBackend::connect_system()
                        .await?
                        .observe_panu(&peer)
                        .await?
                }
                None => None,
            };
            Ok(ObservedClient {
                panu,
                owned_addresses,
                owned_profile,
            })
        })
    }

    fn lease_segment_free(&self, network: NetworkId) -> BackendFuture<'_, bool> {
        Box::pin(async move {
            let plan = Ipv4StarAddressPlan::for_network(network, self.pool)?;
            let active = NetworkManagerBackend::connect_system()
                .await?
                .active_ipv4_prefixes()
                .await?;
            match ensure_ipv4_segment_available(plan.segment, active) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == ErrorKind::AddressConflict => Ok(false),
                Err(error) => Err(error),
            }
        })
    }

    fn reestablish_panu(
        &self,
        network: NetworkId,
        hint: Option<PeerHandle>,
    ) -> BackendFuture<'_, JoinPanLink> {
        Box::pin(async move {
            self.require_open()?;
            let bluez = BluezBackend::connect_system().await?;
            if let Some(hint) = hint {
                // The recorded device first. A stale hint just fails over to the locator.
                let attempt = establish_panu_with(
                    &HintLocator(hint),
                    network,
                    |peer| bluez.connect_panu(peer),
                    |attachment| {
                        let bluez = &bluez;
                        async move { bluez.disconnect_matching_panu(&attachment).await }
                    },
                )
                .await;
                if let Ok(link) = attempt {
                    return Ok(link);
                }
            }
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

    fn apply_lease(
        &self,
        network: NetworkId,
        link: &JoinPanLink,
    ) -> BackendFuture<'_, JoinIpLease> {
        let interface = link.interface().clone();
        Box::pin(async move {
            self.require_open()?;
            let plan = Ipv4StarAddressPlan::for_network(network, self.pool)?;
            let address = InterfaceAddress {
                interface,
                prefix: plan.first_client,
                owner: network,
            };
            NetworkManagerBackend::connect_system()
                .await?
                .ensure_address(address.clone())
                .await?;
            Ok(JoinIpLease::new(address))
        })
    }

    fn start_supervisor(
        &self,
        network: NetworkId,
        host: ClientHost,
        link: JoinPanLink,
        lease: JoinIpLease,
    ) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.require_open()?;
            // The pin comes from the member stores, never from the connection or the hint.
            let expected = PinnedControlTlsPeer::new(network, host.node, host.fingerprint)
                .map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::AuthenticationFailed,
                        "the approved host identity cannot be pinned",
                        error.to_string(),
                    )
                })?;
            let attachment = link.attachment().clone();
            start_supervised_control(
                &self.supervisors,
                self.control.clone(),
                self.pool,
                expected,
                link,
                lease,
            )
            .await?;
            if let Ok(mut retained) = self.retained.lock() {
                retained.insert(network, attachment);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use blueroute_core::{NodeCapabilities, NodeId};
    use blueroute_linux::{NetworkInterfaceHandle, NetworkMembershipStore, PanAttachment, PanRole};
    use blueroute_protocol::peer_pin::SpkiSha256;
    use blueroute_protocol::session::ControlSessionRegistry;

    use super::*;
    use crate::{
        ControlTlsIdentityStore, DurablePeerTrustOperations, LinuxJoinRuntime, PeerPinStore,
    };

    fn control(label: &str) -> JoinControlConfig {
        let root = std::env::temp_dir().join(format!(
            "blueroute-client-runtime-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let local = NodeId::from_bytes([0x71; 16]);
        JoinControlConfig {
            local: Arc::new(
                ControlTlsIdentityStore::new(root.join("identity"))
                    .load_or_create(local)
                    .unwrap(),
            ),
            trust: Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
                NetworkMembershipStore::new(root.join("memberships")),
                PeerPinStore::new(root.join("pins")),
            )),
            registry: Arc::new(Mutex::new(ControlSessionRegistry::new())),
            capabilities: NodeCapabilities::default(),
        }
    }

    fn link() -> JoinPanLink {
        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_00_00_00_00_01").unwrap();
        JoinPanLink::new(
            peer.clone(),
            PanAttachment {
                role: PanRole::Panu,
                interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
                peer: Some(peer),
            },
        )
        .unwrap()
    }

    #[test]
    fn without_a_control_configuration_there_is_no_client_runtime() {
        assert!(LinuxJoinRuntime::default().client_runtime().is_none());
    }

    #[test]
    fn every_mutating_call_is_refused_while_the_gate_is_closed_and_touches_nothing() {
        let runtime = LinuxJoinRuntime::default()
            .with_control(control("closed"))
            .client_runtime()
            .expect("control configured");
        let network = NetworkId::from_bytes([0x72; 16]);
        let host = ClientHost {
            node: NodeId::from_bytes([0x73; 16]),
            fingerprint: SpkiSha256::from_verified_certificate_digest([0x74; 32]).unwrap(),
        };
        let refusals = [
            futures_lite::future::block_on(runtime.reestablish_panu(network, None))
                .map(|_| ())
                .unwrap_err(),
            futures_lite::future::block_on(runtime.apply_lease(network, &link()))
                .map(|_| ())
                .unwrap_err(),
            futures_lite::future::block_on(runtime.start_supervisor(
                network,
                host,
                link(),
                JoinIpLease::new(InterfaceAddress {
                    interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
                    prefix:
                        blueroute_core::IpPrefix::new("192.168.77.2".parse().unwrap(), 24).unwrap(),
                    owner: network,
                }),
            ))
            .unwrap_err(),
        ];
        for error in refusals {
            assert_eq!(error.kind(), ErrorKind::CapabilityUnavailable);
        }
        assert!(runtime.supervisors.state(network).is_none());
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::future::Future;
use std::io::Read;
use std::net::{SocketAddrV4, TcpListener};
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use blueroute_core::{
    CoreError, DaemonConfig, DisplayName, ErrorKind, Ipv4StarAddressPlan, MembershipRegistry,
    MembershipState, NetworkId, NetworkMembership, NodeCapabilities, ensure_ipv4_segment_available,
};
use blueroute_linux::{
    ActiveIpv4Prefix, BluetoothBackend, BluezBackend, InterfaceAddress, IpNetworkBackend,
    IpNetworkObservationBackend, NetworkAdvertisement, NetworkInterfaceHandle,
    NetworkManagerBackend, NetworkMembershipStore, NetworkStateBackend, PanBackend,
};
use blueroute_protocol::{DiagnosticInterface, NetworkSummary};

pub type NetworkOperationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

const MAX_ADDRESS_CONFLICT_RETRIES: usize = 16;

pub trait NetworkOperations: Send + Sync {
    /// Read the current owned NAP endpoint without opening a second socket.
    /// The result is a snapshot, not peer identity or an enduring lease;
    /// a caller must recheck ownership before accepting control traffic.
    fn observed_owned_control_endpoint(&self) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
        Box::pin(async { Ok(None) })
    }

    /// Bind an owned, nonblocking listener with its immutable NAP identity
    /// and endpoint. The caller must still re-observe ownership before each
    /// accept, perform pinned mutual TLS and close sessions during teardown.
    fn bind_guarded_control_listener(
        &self,
    ) -> NetworkOperationFuture<'_, Option<crate::ObservedControlListener>> {
        Box::pin(async { Ok(None) })
    }

    /// Return a TCP listener only for the currently active, independently
    /// observed NAP host address. A missing provider is NOT a wildcard bind.
    /// Merely binding does not activate authenticated peer sessions.
    fn bind_owned_control_listener(&self) -> NetworkOperationFuture<'_, Option<TcpListener>> {
        Box::pin(async { Ok(None) })
    }

    fn create_network(&self, name: DisplayName) -> NetworkOperationFuture<'_, NetworkId>;
    fn leave_network(&self) -> NetworkOperationFuture<'_, ()>;
    fn reconcile_on_startup(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>>;
    /// Only return independently observed backend-owned interface addresses.
    /// None means no observation provider is available; it is not an empty
    /// interface inventory. No peer identity is inferred from an IP address.
    fn diagnostic_interfaces(
        &self,
    ) -> NetworkOperationFuture<'_, Option<Vec<DiagnosticInterface>>> {
        Box::pin(async { Ok(None) })
    }
    fn start_discovery(&self) -> NetworkOperationFuture<'_, ()>;
    fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()>;
}

pub trait NetworkIdGenerator: Send + Sync {
    fn generate(&self) -> Result<NetworkId, CoreError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemNetworkIdGenerator;

impl NetworkIdGenerator for SystemNetworkIdGenerator {
    fn generate(&self) -> Result<NetworkId, CoreError> {
        let mut bytes = [0_u8; 16];
        let mut random = File::open("/dev/urandom").map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to open the Linux random source for a network identity",
                error.to_string(),
            )
        })?;
        random.read_exact(&mut bytes).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to generate a BlueRoute network identity",
                error.to_string(),
            )
        })?;
        Ok(NetworkId::from_bytes(bytes))
    }
}

pub trait StarHostRuntime: Send + Sync {
    /// A transition-safe, read-only snapshot of the active NAP control IP.
    fn observed_owned_control_endpoint(
        &self,
        _config: DaemonConfig,
    ) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
        Box::pin(async { Ok(None) })
    }

    /// Production binds under the host transition guard, returning a socket
    /// paired with the exact network identity and backend-owned endpoint.
    fn bind_guarded_control_listener(
        &self,
        _config: DaemonConfig,
    ) -> NetworkOperationFuture<'_, Option<crate::ObservedControlListener>> {
        Box::pin(async { Ok(None) })
    }

    /// Active-host backend boundary. The production implementation rechecks
    /// NetworkManager-owned addresses before binding; fakes need no listener.
    fn bind_owned_control_listener(
        &self,
        _config: DaemonConfig,
    ) -> NetworkOperationFuture<'_, Option<TcpListener>> {
        Box::pin(async { Ok(None) })
    }

    fn diagnostic_interfaces(
        &self,
    ) -> NetworkOperationFuture<'_, Option<Vec<DiagnosticInterface>>> {
        Box::pin(async { Ok(None) })
    }

    fn start_host(
        &self,
        network: NetworkId,
        bridge: NetworkInterfaceHandle,
        address: InterfaceAddress,
    ) -> NetworkOperationFuture<'_, ()>;

    fn stop_host(
        &self,
        network: NetworkId,
        bridge: NetworkInterfaceHandle,
        address: InterfaceAddress,
    ) -> NetworkOperationFuture<'_, ()>;
}

#[derive(Default)]
pub struct LinuxStarHostRuntime {
    active: Mutex<Option<ActiveLinuxStarHost>>,
    transition: AtomicBool,
}

#[derive(Clone)]
struct ActiveLinuxStarHost {
    network: NetworkId,
    bridge: NetworkInterfaceHandle,
    address: InterfaceAddress,
    adapter: blueroute_linux::AdapterHandle,
    advertisement: NetworkAdvertisement,
    bluez: BluezBackend,
    network_backend: NetworkManagerBackend,
}

impl LinuxStarHostRuntime {
    /// Caller must hold the NAP transition guard across the backend query.
    /// Both observation and binding use exactly the same ownership checks.
    async fn observed_owned_host_addresses(
        &self,
        config: &DaemonConfig,
    ) -> Result<Option<(NetworkId, Vec<InterfaceAddress>)>, CoreError> {
        let active = self.active.lock().map_err(lock_error)?.clone();
        let Some(active) = active else {
            return Ok(None);
        };
        let planned = Ipv4StarAddressPlan::for_network(active.network, config.ipv4_address_pool)?;
        if active.address.owner != active.network
            || active.address.interface != active.bridge
            || active.address.prefix != planned.host
        {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "active NAP runtime does not match its planned host address",
            ));
        }
        let addresses = active.network_backend.addresses().await?;
        Ok(Some((active.network, addresses)))
    }
}

// Idempotent host startup requires the entire owned runtime identity to match.
// A matching logical NetworkId alone must not silently accept a different
// bridge or address after a configuration change or an inconsistent restore.
fn same_host_ownership(
    active_network: NetworkId,
    active_bridge: &NetworkInterfaceHandle,
    active_address: &InterfaceAddress,
    requested_network: NetworkId,
    requested_bridge: &NetworkInterfaceHandle,
    requested_address: &InterfaceAddress,
) -> bool {
    active_network == requested_network
        && active_bridge == requested_bridge
        && active_address == requested_address
}

impl StarHostRuntime for LinuxStarHostRuntime {
    fn bind_guarded_control_listener(
        &self,
        config: DaemonConfig,
    ) -> NetworkOperationFuture<'_, Option<crate::ObservedControlListener>> {
        Box::pin(async move {
            // The network identity, fresh backend observation, TCP bind and
            // listener construction all occur before startup/teardown can
            // acquire the same host transition guard.
            let _transition = TransitionGuard::claim(&self.transition)?;
            let Some((network, addresses)) = self.observed_owned_host_addresses(&config).await?
            else {
                return Ok(None);
            };
            let endpoint = crate::owned_nap_control_address(network, &config, &addresses)?;
            let listener = crate::bind_owned_nap_control_listener(network, &config, &addresses)?;
            let owned = crate::ObservedControlListener::new(network, endpoint, listener)?;
            Ok(Some(owned))
        })
    }

    fn observed_owned_control_endpoint(
        &self,
        config: DaemonConfig,
    ) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
        Box::pin(async move {
            let _transition = TransitionGuard::claim(&self.transition)?;
            let Some((network, addresses)) = self.observed_owned_host_addresses(&config).await?
            else {
                return Ok(None);
            };
            let address = crate::owned_nap_control_address(network, &config, &addresses)?;
            Ok(Some(address))
        })
    }

    fn bind_owned_control_listener(
        &self,
        config: DaemonConfig,
    ) -> NetworkOperationFuture<'_, Option<TcpListener>> {
        Box::pin(async move {
            // Serialize both the backend observation and TCP bind with NAP
            // startup/teardown; an unlocked snapshot could become stale.
            let _transition = TransitionGuard::claim(&self.transition)?;
            let Some((network, addresses)) = self.observed_owned_host_addresses(&config).await?
            else {
                return Ok(None);
            };
            let listener = crate::bind_owned_nap_control_listener(network, &config, &addresses)?;
            Ok(Some(listener))
        })
    }

    fn diagnostic_interfaces(
        &self,
    ) -> NetworkOperationFuture<'_, Option<Vec<DiagnosticInterface>>> {
        Box::pin(async move {
            // Do not observe a partially started or torn-down NAP host.
            // The fresh backend address query belongs to this transition.
            let _transition = TransitionGuard::claim(&self.transition)?;
            let active = self.active.lock().map_err(lock_error)?.clone();
            // An idle daemon cannot report host runtime observations; it
            // must not treat an unknown inventory as an observed empty list.
            let Some(active) = active else {
                return Ok(None);
            };
            let addresses = active.network_backend.addresses().await?;
            Ok(Some(owned_interface_diagnostics(
                active.network,
                &addresses,
            )))
        })
    }

    fn start_host(
        &self,
        network: NetworkId,
        bridge: NetworkInterfaceHandle,
        address: InterfaceAddress,
    ) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async move {
            let _transition = TransitionGuard::claim(&self.transition)?;
            {
                let active = self.active.lock().map_err(lock_error)?;
                if let Some(active) = active.as_ref() {
                    return if same_host_ownership(
                        active.network,
                        &active.bridge,
                        &active.address,
                        network,
                        &bridge,
                        &address,
                    ) {
                        Ok(())
                    } else {
                        Err(CoreError::new(
                            ErrorKind::InvalidState,
                            "a different BlueRoute host configuration is already active",
                        ))
                    };
                }
            }

            let bluez = BluezBackend::connect_system().await?;
            let network_backend = NetworkManagerBackend::connect_system().await?;
            let active_prefixes = network_backend
                .active_ipv4_prefixes_with_interfaces()
                .await?;
            let owned_addresses = network_backend.addresses().await?;
            let unowned_prefixes = active_prefix_conflicts_after_restart(
                network,
                &bridge,
                &address,
                &owned_addresses,
                &active_prefixes,
            );
            ensure_ipv4_segment_available(address.prefix, unowned_prefixes)?;
            let adapter = select_powered_adapter(&bluez).await?;

            network_backend
                .ensure_bridge(network, bridge.clone())
                .await?;

            if let Err(error) = network_backend.ensure_address(address.clone()).await {
                return Err(rollback_setup(
                    error,
                    &bluez,
                    &network_backend,
                    network,
                    &adapter,
                    &bridge,
                    &address,
                    false,
                    false,
                )
                .await);
            }

            if let Err(error) = bluez.start_nap(adapter.clone(), bridge.clone()).await {
                return Err(rollback_setup(
                    error,
                    &bluez,
                    &network_backend,
                    network,
                    &adapter,
                    &bridge,
                    &address,
                    true,
                    true,
                )
                .await);
            }

            let advertisement = match bluez
                .start_network_advertisement(adapter.clone(), network)
                .await
            {
                Ok(advertisement) => advertisement,
                Err(error) => {
                    return Err(rollback_setup(
                        error,
                        &bluez,
                        &network_backend,
                        network,
                        &adapter,
                        &bridge,
                        &address,
                        true,
                        true,
                    )
                    .await);
                }
            };

            let active_host = ActiveLinuxStarHost {
                network,
                bridge,
                address,
                adapter,
                advertisement,
                bluez,
                network_backend,
            };
            let install_error = {
                match self.active.lock() {
                    Ok(mut active) if active.is_none() => {
                        *active = Some(active_host.clone());
                        None
                    }
                    Ok(_) => Some(CoreError::new(
                        ErrorKind::InvalidState,
                        "BlueRoute host runtime changed unexpectedly while creating a network",
                    )),
                    Err(lock) => Some(lock_error(lock)),
                }
            };
            if let Some(error) = install_error {
                Err(rollback_active_host(error, &active_host).await)
            } else {
                Ok(())
            }
        })
    }

    fn stop_host(
        &self,
        network: NetworkId,
        bridge: NetworkInterfaceHandle,
        address: InterfaceAddress,
    ) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async move {
            let _transition = TransitionGuard::claim(&self.transition)?;
            let active = {
                let slot = self.active.lock().map_err(lock_error)?;
                match slot.as_ref() {
                    None => {
                        return Err(CoreError::new(
                            ErrorKind::InvalidState,
                            "cannot confirm ownership of the persisted BlueRoute host runtime",
                        ));
                    }
                    Some(active) if active.network != network => {
                        return Err(CoreError::new(
                            ErrorKind::InvalidState,
                            "refusing to stop a BlueRoute star owned by another network",
                        ));
                    }
                    Some(active) if active.bridge != bridge || active.address != address => {
                        return Err(CoreError::new(
                            ErrorKind::InvalidState,
                            "refusing to stop a BlueRoute star with mismatched owned runtime state",
                        ));
                    }
                    Some(active) => active.clone(),
                }
            };

            cleanup_active_host(&active).await?;

            let mut slot = self.active.lock().map_err(lock_error)?;
            if slot
                .as_ref()
                .is_some_and(|current| current.network == network)
            {
                *slot = None;
                Ok(())
            } else {
                Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "BlueRoute host runtime changed unexpectedly during cleanup",
                ))
            }
        })
    }
}

#[derive(Clone)]
struct ActiveNetworkDiscovery {
    adapter: blueroute_linux::AdapterHandle,
    bluez: BluezBackend,
}

pub struct SingleStarNetworkOperations<R = LinuxStarHostRuntime, G = SystemNetworkIdGenerator> {
    store: NetworkMembershipStore,
    config: DaemonConfig,
    capabilities: NodeCapabilities,
    runtime: R,
    generator: G,
    creating: AtomicBool,
    discovery: Mutex<Option<ActiveNetworkDiscovery>>,
    discovery_transition: AtomicBool,
}

impl SingleStarNetworkOperations<LinuxStarHostRuntime, SystemNetworkIdGenerator> {
    pub fn new(
        store: NetworkMembershipStore,
        config: DaemonConfig,
        capabilities: NodeCapabilities,
    ) -> Self {
        Self::with_runtime_and_generator(
            store,
            config,
            capabilities,
            LinuxStarHostRuntime::default(),
            SystemNetworkIdGenerator,
        )
    }
}

impl<R, G> SingleStarNetworkOperations<R, G>
where
    R: StarHostRuntime,
    G: NetworkIdGenerator,
{
    pub fn with_runtime_and_generator(
        store: NetworkMembershipStore,
        config: DaemonConfig,
        capabilities: NodeCapabilities,
        runtime: R,
        generator: G,
    ) -> Self {
        Self {
            store,
            config,
            capabilities,
            runtime,
            generator,
            creating: AtomicBool::new(false),
            discovery: Mutex::new(None),
            discovery_transition: AtomicBool::new(false),
        }
    }

    fn create_network_inner(&self, name: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        Box::pin(async move {
            let _creating = TransitionGuard::claim(&self.creating)?;
            ensure_nap_capability(&self.capabilities)?;

            let mut registry = self.store.load()?;
            if current_network(&registry)?.is_some() {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "this daemon already belongs to a BlueRoute network",
                ));
            }

            let mut attempted = BTreeSet::new();
            let mut last_conflict = None;
            let (network, bridge, address) = loop {
                if attempted.len() >= MAX_ADDRESS_CONFLICT_RETRIES {
                    let diagnostic = last_conflict
                        .as_ref()
                        .map(|error: &CoreError| error.to_string())
                        .unwrap_or_else(|| "no conflict diagnostic was recorded".to_owned());
                    return Err(CoreError::with_diagnostic(
                        ErrorKind::AddressConflict,
                        "failed to select a conflict-free BlueRoute IPv4 segment",
                        format!(
                            "attempted {} network identities; last conflict: {diagnostic}",
                            attempted.len()
                        ),
                    ));
                }

                let network = unique_network_id(&registry, &attempted, &self.generator)?;
                attempted.insert(network);
                let bridge = bridge_name(network)?;
                let address = local_star_address(network, &self.config)?;
                match self
                    .runtime
                    .start_host(network, bridge.clone(), address.clone())
                    .await
                {
                    Ok(()) => break (network, bridge, address),
                    Err(error) if error.kind() == ErrorKind::AddressConflict => {
                        last_conflict = Some(error);
                    }
                    Err(error) => return Err(error),
                }
            };

            let mut membership = NetworkMembership::new(network, name);
            membership.state = membership
                .state
                .transition(MembershipState::Joining)?
                .transition(MembershipState::Member)?;
            registry.remember_network(membership);

            if let Err(persist_error) = self.store.save(&registry) {
                return match self.runtime.stop_host(network, bridge, address).await {
                    Ok(()) => Err(persist_error),
                    Err(cleanup_error) => Err(CoreError::with_diagnostic(
                        ErrorKind::PersistenceError,
                        persist_error.message(),
                        format!(
                            "durable membership commit failed and runtime rollback also failed: {cleanup_error}; rollback diagnostic={:?}",
                            cleanup_error.diagnostic()
                        ),
                    )),
                };
            }

            Ok(network)
        })
    }
}

impl<R, G> NetworkOperations for SingleStarNetworkOperations<R, G>
where
    R: StarHostRuntime,
    G: NetworkIdGenerator,
{
    fn bind_guarded_control_listener(
        &self,
    ) -> NetworkOperationFuture<'_, Option<crate::ObservedControlListener>> {
        Box::pin(async move {
            // A live NAP backend is insufficient authority if durable local
            // membership was revoked or replaced. Check before binding and
            // again after the asynchronous guarded bind completes.
            let Some(network) = current_network(&self.store.load()?)? else {
                return Ok(None);
            };
            let Some(listener) = self
                .runtime
                .bind_guarded_control_listener(self.config.clone())
                .await?
            else {
                return Ok(None);
            };
            if listener.network() != network
                || current_network(&self.store.load()?)? != Some(network)
            {
                // Dropping the listener closes its queued unauthenticated
                // candidates; never expose a socket from a stale membership.
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP control listener is not backed by current durable membership",
                ));
            }
            Ok(Some(listener))
        })
    }

    fn observed_owned_control_endpoint(&self) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
        Box::pin(async move {
            // A live NAP backend alone does not establish durable membership.
            // The caller must still recheck both facts before every accept.
            let Some(network) = current_network(&self.store.load()?)? else {
                return Ok(None);
            };
            let observed = self
                .runtime
                .observed_owned_control_endpoint(self.config.clone())
                .await?;
            if current_network(&self.store.load()?)? != Some(network) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP control endpoint lost durable membership during observation",
                ));
            }
            let Some(endpoint) = observed else {
                return Ok(None);
            };
            let planned = Ipv4StarAddressPlan::for_network(network, self.config.ipv4_address_pool)?;
            let std::net::IpAddr::V4(ip) = planned.host.address else {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP control host must have an IPv4 address",
                ));
            };
            if endpoint != SocketAddrV4::new(ip, crate::CONTROL_TCP_PORT) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP control endpoint does not match durable host network",
                ));
            }
            Ok(Some(endpoint))
        })
    }

    fn bind_owned_control_listener(&self) -> NetworkOperationFuture<'_, Option<TcpListener>> {
        Box::pin(async move {
            // Legacy raw-socket access must not bypass the durable membership
            // checks enforced by the guarded listener API. This still does
            // not authenticate clients; production must use the guarded API.
            let Some(network) = current_network(&self.store.load()?)? else {
                return Ok(None);
            };
            let listener = self
                .runtime
                .bind_owned_control_listener(self.config.clone())
                .await?;
            if current_network(&self.store.load()?)? != Some(network) {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP control bind lost durable membership",
                ));
            }
            let Some(listener) = listener else {
                return Ok(None);
            };
            let planned = Ipv4StarAddressPlan::for_network(network, self.config.ipv4_address_pool)?;
            let std::net::IpAddr::V4(ip) = planned.host.address else {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP control host must have an IPv4 address",
                ));
            };
            let expected = std::net::SocketAddr::V4(SocketAddrV4::new(ip, crate::CONTROL_TCP_PORT));
            if listener.local_addr().map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::PanFailure,
                    "cannot inspect raw NAP control listener",
                    error.to_string(),
                )
            })? != expected
            {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "raw NAP control listener does not match durable host network",
                ));
            }
            Ok(Some(listener))
        })
    }

    fn diagnostic_interfaces(
        &self,
    ) -> NetworkOperationFuture<'_, Option<Vec<DiagnosticInterface>>> {
        self.runtime.diagnostic_interfaces()
    }

    fn create_network(&self, name: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        self.create_network_inner(name)
    }

    fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async move {
            let _transition = TransitionGuard::claim(&self.creating)?;
            let mut registry = self.store.load()?;
            let Some(network) = current_network(&registry)? else {
                return Ok(());
            };
            if registry.network(&network).is_none() {
                return Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "active BlueRoute membership is missing from durable state",
                ));
            }
            let bridge = bridge_name(network)?;
            let address = local_star_address(network, &self.config)?;

            self.runtime
                .stop_host(network, bridge.clone(), address.clone())
                .await?;

            let membership = registry.network_mut(&network).ok_or_else(|| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "BlueRoute membership changed during leave",
                )
            })?;
            membership.state = membership
                .state
                .transition(MembershipState::Leaving)?
                .transition(MembershipState::NotMember)?;
            for peer in membership
                .peers()
                .map(|peer| peer.node_id)
                .collect::<Vec<_>>()
            {
                membership.set_peer_member(peer, false);
            }
            if let Err(persist_error) = self.store.save(&registry) {
                return match self.runtime.start_host(network, bridge, address).await {
                    Ok(()) => Err(persist_error),
                    Err(rollback_error) => Err(CoreError::with_diagnostic(
                        ErrorKind::PersistenceError,
                        persist_error.message(),
                        format!(
                            "durable leave failed and host runtime restoration also failed: {rollback_error}"
                        ),
                    )),
                };
            }
            Ok(())
        })
    }

    fn reconcile_on_startup(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async move {
            let _transition = TransitionGuard::claim(&self.creating)?;
            let registry = self.store.load()?;
            let Some(network) = current_network(&registry)? else {
                return Ok(());
            };
            // Production JoinNetwork remains fail-closed, so a persisted local
            // Member currently represents a host created by this daemon.
            // Persisted role must be added before client join is enabled.
            ensure_nap_capability(&self.capabilities)?;
            self.runtime
                .start_host(
                    network,
                    bridge_name(network)?,
                    local_star_address(network, &self.config)?,
                )
                .await
        })
    }

    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>> {
        Box::pin(async move {
            let registry = self.store.load()?;
            let mut networks = BTreeMap::new();
            for membership in registry.networks() {
                let local_member = if membership.state == MembershipState::Member {
                    1
                } else {
                    0
                };
                networks.insert(
                    membership.network_id,
                    NetworkSummary {
                        id: membership.network_id,
                        name: membership.network_name.clone(),
                        member_count: local_member
                            + membership.peers().filter(|peer| peer.is_member()).count() as u32,
                    },
                );
            }

            let discovery = self.discovery.lock().map_err(lock_error)?.clone();
            if let Some(discovery) = discovery {
                let discovered = discovery
                    .bluez
                    .discovered_network_ids(discovery.adapter.clone())
                    .await?;
                merge_discovered_networks(&mut networks, discovered)?;
            }

            Ok(networks.into_values().collect())
        })
    }

    fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async move {
            let _transition = TransitionGuard::claim(&self.discovery_transition)?;
            if self.discovery.lock().map_err(lock_error)?.is_some() {
                return Ok(());
            }

            let bluez = BluezBackend::connect_system().await?;
            let adapter = select_powered_discovery_adapter(&bluez).await?;
            bluez.start_discovery(adapter.clone()).await?;
            let active = ActiveNetworkDiscovery { adapter, bluez };

            let install_error = match self.discovery.lock() {
                Ok(mut slot) if slot.is_none() => {
                    *slot = Some(active.clone());
                    None
                }
                Ok(_) => Some(CoreError::new(
                    ErrorKind::InvalidState,
                    "BlueRoute discovery state changed unexpectedly while starting discovery",
                )),
                Err(error) => Some(lock_error(error)),
            };
            if let Some(error) = install_error {
                return match active.bluez.stop_discovery(active.adapter).await {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(CoreError::with_diagnostic(
                        error.kind(),
                        error.message(),
                        format!(
                            "{}; discovery rollback also failed: {cleanup_error}",
                            error.diagnostic().unwrap_or("no discovery diagnostic")
                        ),
                    )),
                };
            }
            Ok(())
        })
    }

    fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async move {
            let _transition = TransitionGuard::claim(&self.discovery_transition)?;
            let active = self.discovery.lock().map_err(lock_error)?.clone();
            let Some(active) = active else {
                return Ok(());
            };

            active.bluez.stop_discovery(active.adapter.clone()).await?;
            let mut slot = self.discovery.lock().map_err(lock_error)?;
            if slot
                .as_ref()
                .is_some_and(|current| current.adapter == active.adapter)
            {
                *slot = None;
                Ok(())
            } else {
                Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "BlueRoute discovery state changed unexpectedly while stopping discovery",
                ))
            }
        })
    }
}

fn merge_discovered_networks(
    networks: &mut BTreeMap<NetworkId, NetworkSummary>,
    discovered: impl IntoIterator<Item = NetworkId>,
) -> Result<(), CoreError> {
    for network in discovered {
        if let std::collections::btree_map::Entry::Vacant(entry) = networks.entry(network) {
            entry.insert(discovered_network_summary(network)?);
        }
    }
    Ok(())
}

fn active_prefix_conflicts_after_restart(
    network: NetworkId,
    bridge: &NetworkInterfaceHandle,
    address: &InterfaceAddress,
    owned_addresses: &[InterfaceAddress],
    active_prefixes: &[ActiveIpv4Prefix],
) -> Vec<blueroute_core::IpPrefix> {
    let exact_owned_address_exists = owned_addresses.iter().any(|owned| {
        owned.owner == network
            && owned.interface == *bridge
            && owned.interface == address.interface
            && owned.prefix == address.prefix
    });
    active_prefixes
        .iter()
        .filter(|active| {
            !(exact_owned_address_exists
                && active.interface.as_ref() == Some(bridge)
                && active.prefix == address.prefix)
        })
        .map(|active| active.prefix)
        .collect()
}

fn discovered_network_summary(network: NetworkId) -> Result<NetworkSummary, CoreError> {
    let id = network.to_string();
    Ok(NetworkSummary {
        id: network,
        name: DisplayName::new(format!("BlueRoute {}", &id[..8]))?,
        member_count: 0,
    })
}

/// Project only backend-reported, BlueRoute-owned IPv4 address entries for
/// the active NAP network into read-only local diagnostics. This is not a
/// physical link or authenticated peer observation. Foreign and other-network
/// addresses are never attributed to the hosted network.
fn owned_interface_diagnostics(
    network: NetworkId,
    addresses: &[InterfaceAddress],
) -> Vec<DiagnosticInterface> {
    let mut interfaces: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for address in addresses {
        if address.owner != network {
            continue;
        }
        let prefix = format!("{}/{}", address.prefix.address, address.prefix.prefix_len);
        interfaces
            .entry(address.interface.as_str().to_owned())
            .or_default()
            .push(prefix);
    }
    interfaces
        .into_iter()
        .map(|(name, mut addresses)| {
            addresses.sort();
            addresses.dedup();
            DiagnosticInterface { name, addresses }
        })
        .collect()
}

pub fn current_network(registry: &MembershipRegistry) -> Result<Option<NetworkId>, CoreError> {
    let mut members = registry
        .networks()
        .filter(|membership| membership.state == MembershipState::Member)
        .map(|membership| membership.network_id);
    let current = members.next();
    if members.next().is_some() {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "durable state contains more than one active BlueRoute network",
        ));
    }
    Ok(current)
}

fn ensure_nap_capability(capabilities: &NodeCapabilities) -> Result<(), CoreError> {
    match capabilities.can_host_pan() {
        Some(true) => Ok(()),
        Some(false) => Err(CoreError::new(
            ErrorKind::CapabilityUnavailable,
            "this Linux node does not expose BlueZ NAP capability",
        )),
        None => Err(CoreError::new(
            ErrorKind::CapabilityUnavailable,
            "BlueZ NAP capability is unknown; refusing to create a hosted network",
        )),
    }
}

fn unique_network_id(
    registry: &MembershipRegistry,
    attempted: &BTreeSet<NetworkId>,
    generator: &impl NetworkIdGenerator,
) -> Result<NetworkId, CoreError> {
    for _ in 0..16 {
        let candidate = generator.generate()?;
        if registry.network(&candidate).is_none() && !attempted.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(CoreError::new(
        ErrorKind::Internal,
        "failed to generate a unique BlueRoute network identity",
    ))
}

fn bridge_name(network: NetworkId) -> Result<NetworkInterfaceHandle, CoreError> {
    let id = network.to_string();
    NetworkInterfaceHandle::new(format!("brb-{}", &id[..8]))
}

fn local_star_address(
    network: NetworkId,
    config: &DaemonConfig,
) -> Result<InterfaceAddress, CoreError> {
    config.validate()?;
    let plan = Ipv4StarAddressPlan::for_network(network, config.ipv4_address_pool)?;
    Ok(InterfaceAddress {
        interface: bridge_name(network)?,
        prefix: plan.host,
        owner: network,
    })
}

async fn select_powered_adapter(
    bluez: &BluezBackend,
) -> Result<blueroute_linux::AdapterHandle, CoreError> {
    let adapters = bluez.adapters().await?;
    if let Some(adapter) = adapters.into_iter().find(|adapter| adapter.powered) {
        return Ok(adapter.handle);
    }
    Err(CoreError::new(
        ErrorKind::AdapterDisabled,
        "no powered Bluetooth adapter is available to host a BlueRoute NAP",
    ))
}

async fn select_powered_discovery_adapter(
    bluez: &BluezBackend,
) -> Result<blueroute_linux::AdapterHandle, CoreError> {
    let adapters = bluez.adapters().await?;
    if let Some(adapter) = adapters.into_iter().find(|adapter| adapter.powered) {
        return Ok(adapter.handle);
    }
    Err(CoreError::new(
        ErrorKind::AdapterDisabled,
        "no powered Bluetooth adapter is available for BlueRoute discovery",
    ))
}

#[allow(clippy::too_many_arguments)]
async fn rollback_setup(
    original: CoreError,
    bluez: &BluezBackend,
    network_backend: &NetworkManagerBackend,
    network: NetworkId,
    adapter: &blueroute_linux::AdapterHandle,
    bridge: &NetworkInterfaceHandle,
    address: &InterfaceAddress,
    cleanup_nap: bool,
    cleanup_address: bool,
) -> CoreError {
    let mut failures = Vec::new();
    if cleanup_nap && let Err(error) = bluez.stop_nap(adapter.clone()).await {
        failures.push(format!("stop NAP: {error}"));
    }
    if cleanup_address && let Err(error) = network_backend.remove_address(address.clone()).await {
        failures.push(format!("remove address: {error}"));
    }
    if let Err(error) = network_backend
        .remove_owned_interface(network, bridge.clone())
        .await
    {
        failures.push(format!("remove bridge: {error}"));
    }
    attach_cleanup_failures(original, failures)
}

async fn rollback_active_host(original: CoreError, active: &ActiveLinuxStarHost) -> CoreError {
    match cleanup_active_host(active).await {
        Ok(()) => original,
        Err(cleanup) => CoreError::with_diagnostic(
            original.kind(),
            original.message(),
            format!(
                "{}; runtime rollback also failed: {cleanup}; cleanup diagnostic={:?}",
                original.diagnostic().unwrap_or("no original diagnostic"),
                cleanup.diagnostic()
            ),
        ),
    }
}

async fn cleanup_active_host(active: &ActiveLinuxStarHost) -> Result<(), CoreError> {
    let mut failures = Vec::new();
    if let Err(error) = active
        .bluez
        .stop_network_advertisement(active.advertisement.clone())
        .await
    {
        failures.push(format!("stop discovery advertisement: {error}"));
    }
    if let Err(error) = active.bluez.stop_nap(active.adapter.clone()).await {
        failures.push(format!("stop NAP: {error}"));
    }
    if let Err(error) = active
        .network_backend
        .remove_address(active.address.clone())
        .await
    {
        failures.push(format!("remove address: {error}"));
    }
    if let Err(error) = active
        .network_backend
        .remove_owned_interface(active.network, active.bridge.clone())
        .await
    {
        failures.push(format!("remove bridge: {error}"));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(CoreError::with_diagnostic(
            ErrorKind::Internal,
            "failed to fully clean up BlueRoute host runtime state",
            failures.join("; "),
        ))
    }
}

fn attach_cleanup_failures(original: CoreError, failures: Vec<String>) -> CoreError {
    if failures.is_empty() {
        return original;
    }
    CoreError::with_diagnostic(
        original.kind(),
        original.message(),
        format!(
            "{}; runtime rollback failures: {}",
            original.diagnostic().unwrap_or("no original diagnostic"),
            failures.join("; ")
        ),
    )
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::Internal,
        "BlueRoute runtime state lock was poisoned",
        error.to_string(),
    )
}

struct TransitionGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> TransitionGuard<'a> {
    fn claim(flag: &'a AtomicBool) -> Result<Self, CoreError> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "a conflicting BlueRoute network transition is already in progress",
                )
            })?;
        Ok(Self { flag })
    }
}

impl Drop for TransitionGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::net::IpAddr;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use blueroute_core::{CapabilitySource, Sourced};

    use super::*;

    #[derive(Default)]
    struct FakeRuntimeState {
        starts: AtomicUsize,
        stops: AtomicUsize,
        active: AtomicBool,
        fail_start: AtomicBool,
        fail_stop: AtomicBool,
        address_conflicts_remaining: AtomicUsize,
    }

    #[derive(Clone, Default)]
    struct FakeRuntime {
        state: Arc<FakeRuntimeState>,
    }

    impl FakeRuntime {
        fn starts(&self) -> usize {
            self.state.starts.load(Ordering::SeqCst)
        }
    }

    impl StarHostRuntime for FakeRuntime {
        fn start_host(
            &self,
            _network: NetworkId,
            _bridge: NetworkInterfaceHandle,
            _address: InterfaceAddress,
        ) -> NetworkOperationFuture<'_, ()> {
            Box::pin(async move {
                self.state.starts.fetch_add(1, Ordering::SeqCst);
                if self
                    .state
                    .address_conflicts_remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
                {
                    return Err(CoreError::new(
                        ErrorKind::AddressConflict,
                        "fake active-route conflict",
                    ));
                }
                if self.state.fail_start.load(Ordering::SeqCst) {
                    Err(CoreError::new(ErrorKind::PanFailure, "fake NAP failure"))
                } else {
                    self.state.active.store(true, Ordering::SeqCst);
                    Ok(())
                }
            })
        }

        fn stop_host(
            &self,
            _network: NetworkId,
            _bridge: NetworkInterfaceHandle,
            _address: InterfaceAddress,
        ) -> NetworkOperationFuture<'_, ()> {
            Box::pin(async move {
                self.state.stops.fetch_add(1, Ordering::SeqCst);
                if !self.state.active.load(Ordering::SeqCst) {
                    return Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "fake host runtime is not active",
                    ));
                }
                if self.state.fail_stop.load(Ordering::SeqCst) {
                    return Err(CoreError::new(ErrorKind::PanFailure, "fake stop failure"));
                }
                self.state.active.store(false, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    struct SequenceGenerator {
        values: Mutex<VecDeque<NetworkId>>,
    }

    impl SequenceGenerator {
        fn new(values: impl IntoIterator<Item = NetworkId>) -> Self {
            Self {
                values: Mutex::new(values.into_iter().collect()),
            }
        }
    }

    impl NetworkIdGenerator for SequenceGenerator {
        fn generate(&self) -> Result<NetworkId, CoreError> {
            self.values
                .lock()
                .map_err(lock_error)?
                .pop_front()
                .ok_or_else(|| CoreError::new(ErrorKind::Internal, "fake IDs exhausted"))
        }
    }

    fn nap_capabilities(enabled: Option<bool>) -> NodeCapabilities {
        NodeCapabilities {
            nap: enabled.map(|value| Sourced::new(value, CapabilitySource::Measured)),
            ..NodeCapabilities::default()
        }
    }

    fn temp_store(label: &str) -> (std::path::PathBuf, NetworkMembershipStore) {
        let root = std::env::temp_dir().join(format!(
            "blueroute-p6-001-{label}-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("memberships-v1");
        (root, NetworkMembershipStore::new(path))
    }

    static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn repeated_host_start_requires_exact_network_bridge_and_address_ownership() {
        let network = NetworkId::from_bytes([0x31; 16]);
        let other = NetworkId::from_bytes([0x32; 16]);
        let bridge = bridge_name(network).unwrap();
        let address = local_star_address(network, &DaemonConfig::default()).unwrap();
        assert!(same_host_ownership(
            network, &bridge, &address, network, &bridge, &address
        ));
        assert!(!same_host_ownership(
            network, &bridge, &address, other, &bridge, &address
        ));
        let other_bridge = NetworkInterfaceHandle::new("brb-other").unwrap();
        assert!(!same_host_ownership(
            network,
            &bridge,
            &address,
            network,
            &other_bridge,
            &address
        ));
        let mut other_address = address.clone();
        other_address.owner = other;
        assert!(!same_host_ownership(
            network,
            &bridge,
            &address,
            network,
            &bridge,
            &other_address
        ));
        let mut other_address = address.clone();
        other_address.prefix =
            blueroute_core::IpPrefix::new("10.201.99.1".parse().unwrap(), 24).unwrap();
        assert!(!same_host_ownership(
            network,
            &bridge,
            &address,
            network,
            &bridge,
            &other_address
        ));
    }

    #[test]
    fn owned_interface_diagnostics_filter_other_networks_and_stabilize_order() {
        let owner = NetworkId::from_bytes([1; 16]);
        let foreign = NetworkId::from_bytes([2; 16]);
        let entries = vec![
            InterfaceAddress {
                interface: NetworkInterfaceHandle::new("brb-host").unwrap(),
                prefix: blueroute_core::IpPrefix::new("10.72.0.1".parse().unwrap(), 24).unwrap(),
                owner,
            },
            InterfaceAddress {
                interface: NetworkInterfaceHandle::new("eth0").unwrap(),
                prefix: blueroute_core::IpPrefix::new("192.168.0.10".parse().unwrap(), 24).unwrap(),
                owner: foreign,
            },
            InterfaceAddress {
                interface: NetworkInterfaceHandle::new("brb-host").unwrap(),
                prefix: blueroute_core::IpPrefix::new("10.72.0.1".parse().unwrap(), 24).unwrap(),
                owner,
            },
        ];
        assert_eq!(
            owned_interface_diagnostics(owner, &entries),
            vec![DiagnosticInterface {
                name: "brb-host".into(),
                addresses: vec!["10.72.0.1/24".into()],
            }]
        );
        assert!(
            owned_interface_diagnostics(foreign, &entries)
                .iter()
                .all(|item| item.name != "brb-host")
        );
        assert!(owned_interface_diagnostics(NetworkId::from_bytes([3; 16]), &entries).is_empty());
    }

    #[test]
    fn idle_host_cannot_bind_control_listener_through_runtime_or_operations() {
        futures_lite::future::block_on(async {
            let empty_runtime = LinuxStarHostRuntime::default();
            assert!(
                empty_runtime
                    .bind_guarded_control_listener(DaemonConfig::default())
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                empty_runtime
                    .bind_owned_control_listener(DaemonConfig::default())
                    .await
                    .unwrap()
                    .is_none()
            );

            assert!(
                empty_runtime
                    .observed_owned_control_endpoint(DaemonConfig::default())
                    .await
                    .unwrap()
                    .is_none()
            );

            let (root, store) = temp_store("idle-control-listener");
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                FakeRuntime::default(),
                SequenceGenerator::new([]),
            );
            assert!(
                operations
                    .bind_guarded_control_listener()
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                operations
                    .bind_owned_control_listener()
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                operations
                    .observed_owned_control_endpoint()
                    .await
                    .unwrap()
                    .is_none()
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn diagnostic_interfaces_rejects_host_transition_then_recovers_when_idle() {
        futures_lite::future::block_on(async {
            let runtime = LinuxStarHostRuntime::default();
            let transition = TransitionGuard::claim(&runtime.transition).unwrap();
            let error = runtime.diagnostic_interfaces().await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            drop(transition);
            assert!(runtime.diagnostic_interfaces().await.unwrap().is_none());
        });
    }

    #[test]
    fn control_listener_binding_rejects_host_transition_and_recovers_when_idle() {
        futures_lite::future::block_on(async {
            let runtime = LinuxStarHostRuntime::default();
            // A startup or teardown already owns the transition; binding
            // must fail before it can query a stale host address.
            let transition = TransitionGuard::claim(&runtime.transition).unwrap();
            let guarded_error = runtime
                .bind_guarded_control_listener(DaemonConfig::default())
                .await
                .err()
                .unwrap();
            assert_eq!(guarded_error.kind(), ErrorKind::InvalidState);
            let failure = runtime
                .bind_owned_control_listener(DaemonConfig::default())
                .await
                .unwrap_err();
            assert_eq!(failure.kind(), ErrorKind::InvalidState);
            let observed = runtime
                .observed_owned_control_endpoint(DaemonConfig::default())
                .await
                .unwrap_err();
            assert_eq!(observed.kind(), ErrorKind::InvalidState);
            drop(transition);
            assert!(
                runtime
                    .bind_owned_control_listener(DaemonConfig::default())
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                runtime
                    .observed_owned_control_endpoint(DaemonConfig::default())
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn discovered_network_merge_preserves_remembered_metadata() {
        let remembered = NetworkId::from_bytes([0x11; 16]);
        let discovered = NetworkId::from_bytes([0x22; 16]);
        let mut networks = BTreeMap::from([(
            remembered,
            NetworkSummary {
                id: remembered,
                name: DisplayName::new("Remembered name").unwrap(),
                member_count: 1,
            },
        )]);

        merge_discovered_networks(&mut networks, [remembered, discovered, discovered]).unwrap();

        assert_eq!(networks.len(), 2);
        assert_eq!(networks[&remembered].name.as_str(), "Remembered name");
        assert_eq!(networks[&discovered].name.as_str(), "BlueRoute 22222222");
        assert_eq!(networks[&discovered].member_count, 0);
    }

    #[test]
    fn restart_collision_check_ignores_only_exact_owned_interface_address() {
        let network = NetworkId::from_bytes([0x61; 16]);
        let bridge = NetworkInterfaceHandle::new("brb-61616161").unwrap();
        let foreign = NetworkInterfaceHandle::new("eth0").unwrap();
        let address = InterfaceAddress {
            interface: bridge.clone(),
            prefix: blueroute_core::IpPrefix::new("10.201.44.0".parse().unwrap(), 24).unwrap(),
            owner: network,
        };
        let owned = vec![address.clone()];
        let active = vec![
            ActiveIpv4Prefix {
                interface: Some(bridge.clone()),
                prefix: address.prefix,
            },
            ActiveIpv4Prefix {
                interface: Some(foreign),
                prefix: blueroute_core::IpPrefix::new("10.201.44.0".parse().unwrap(), 24).unwrap(),
            },
        ];

        assert_eq!(
            active_prefix_conflicts_after_restart(network, &bridge, &address, &owned, &active),
            vec![address.prefix]
        );
        assert_eq!(
            active_prefix_conflicts_after_restart(network, &bridge, &address, &[], &active),
            vec![address.prefix, address.prefix]
        );
    }

    #[test]
    fn create_network_commits_membership_only_after_runtime_setup() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("commit");
            let network = NetworkId::from_bytes([7; 16]);
            let runtime = FakeRuntime::default();
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([network]),
            );

            let created = operations
                .create_network(DisplayName::new("Workshop").unwrap())
                .await
                .unwrap();
            assert_eq!(created, network);
            assert_eq!(runtime.starts(), 1);
            let networks = operations.list_networks().await.unwrap();
            assert_eq!(networks.len(), 1);
            assert_eq!(networks[0].id, network);
            assert_eq!(networks[0].name.as_str(), "Workshop");
            assert_eq!(networks[0].member_count, 1);

            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn leave_stops_owned_host_clears_membership_and_retains_peer_trust() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("leave");
            let network = NetworkId::from_bytes([0x31; 16]);
            let peer = blueroute_core::NodeId::from_bytes([0x32; 16]);
            let runtime = FakeRuntime::default();
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([network]),
            );
            operations
                .create_network(DisplayName::new("Leave me").unwrap())
                .await
                .unwrap();
            let mut registry = operations.store.load().unwrap();
            let membership = registry.network_mut(&network).unwrap();
            membership.trust_peer(peer);
            membership.set_peer_member(peer, true);
            operations.store.save(&registry).unwrap();

            operations.leave_network().await.unwrap();
            operations.leave_network().await.unwrap();

            let restored = operations.store.load().unwrap();
            let membership = restored.network(&network).unwrap();
            assert_eq!(membership.state, MembershipState::NotMember);
            assert!(membership.is_peer_trusted(&peer));
            assert!(!membership.is_peer_member(&peer));
            assert_eq!(current_network(&restored).unwrap(), None);
            assert_eq!(runtime.state.stops.load(Ordering::SeqCst), 1);
            assert!(!runtime.state.active.load(Ordering::SeqCst));
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn leave_refuses_to_clear_membership_when_runtime_is_not_owned() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("leave-not-owned");
            let network = NetworkId::from_bytes([0x41; 16]);
            let runtime = FakeRuntime::default();
            let mut registry = MembershipRegistry::default();
            let mut membership =
                NetworkMembership::new(network, DisplayName::new("Retained").unwrap());
            membership.state = MembershipState::Joining
                .transition(MembershipState::Member)
                .unwrap();
            registry.remember_network(membership);
            store.save(&registry).unwrap();
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime,
                SequenceGenerator::new([]),
            );

            let error = operations.leave_network().await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert_eq!(
                current_network(&operations.store.load().unwrap()).unwrap(),
                Some(network)
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn startup_reconciliation_restores_host_from_durable_membership() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("startup-reconcile");
            let network = NetworkId::from_bytes([0x71; 16]);
            let mut registry = MembershipRegistry::default();
            let mut membership =
                NetworkMembership::new(network, DisplayName::new("Restore me").unwrap());
            membership.state = MembershipState::Joining
                .transition(MembershipState::Member)
                .unwrap();
            registry.remember_network(membership);
            store.save(&registry).unwrap();
            let runtime = FakeRuntime::default();
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([]),
            );

            operations.reconcile_on_startup().await.unwrap();

            assert_eq!(runtime.starts(), 1);
            assert!(runtime.state.active.load(Ordering::SeqCst));
            assert_eq!(
                current_network(&operations.store.load().unwrap()).unwrap(),
                Some(network)
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn startup_reconciliation_keeps_membership_when_runtime_restore_fails() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("startup-reconcile-failure");
            let network = NetworkId::from_bytes([0x72; 16]);
            let mut registry = MembershipRegistry::default();
            let mut membership =
                NetworkMembership::new(network, DisplayName::new("Retry restore").unwrap());
            membership.state = MembershipState::Joining
                .transition(MembershipState::Member)
                .unwrap();
            registry.remember_network(membership);
            store.save(&registry).unwrap();
            let runtime = FakeRuntime::default();
            runtime.state.fail_start.store(true, Ordering::SeqCst);
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([]),
            );

            assert_eq!(
                operations.reconcile_on_startup().await.unwrap_err().kind(),
                ErrorKind::PanFailure
            );
            assert_eq!(runtime.starts(), 1);
            assert_eq!(
                current_network(&operations.store.load().unwrap()).unwrap(),
                Some(network)
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn failed_host_cleanup_keeps_membership_for_retry() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("leave-cleanup-failure");
            let network = NetworkId::from_bytes([0x51; 16]);
            let runtime = FakeRuntime::default();
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([network]),
            );
            operations
                .create_network(DisplayName::new("Retry leave").unwrap())
                .await
                .unwrap();
            runtime.state.fail_stop.store(true, Ordering::SeqCst);

            assert_eq!(
                operations.leave_network().await.unwrap_err().kind(),
                ErrorKind::PanFailure
            );
            assert_eq!(
                current_network(&operations.store.load().unwrap()).unwrap(),
                Some(network)
            );
            assert!(runtime.state.active.load(Ordering::SeqCst));
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn unavailable_nap_fails_before_runtime_mutation() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("no-nap");
            let runtime = FakeRuntime::default();
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(false)),
                runtime.clone(),
                SequenceGenerator::new([NetworkId::from_bytes([8; 16])]),
            );

            let error = operations
                .create_network(DisplayName::new("Unsupported").unwrap())
                .await
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::CapabilityUnavailable);
            assert_eq!(runtime.starts(), 0);
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn runtime_failure_does_not_persist_membership() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("runtime-fail");
            let runtime = FakeRuntime::default();
            runtime.state.fail_start.store(true, Ordering::SeqCst);
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([NetworkId::from_bytes([9; 16])]),
            );

            let error = operations
                .create_network(DisplayName::new("Failure").unwrap())
                .await
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::PanFailure);
            assert_eq!(runtime.starts(), 1);
            assert!(operations.list_networks().await.unwrap().is_empty());
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn second_create_is_rejected_without_a_second_runtime_start() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("duplicate");
            let runtime = FakeRuntime::default();
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([
                    NetworkId::from_bytes([10; 16]),
                    NetworkId::from_bytes([11; 16]),
                ]),
            );

            operations
                .create_network(DisplayName::new("First").unwrap())
                .await
                .unwrap();
            let error = operations
                .create_network(DisplayName::new("Second").unwrap())
                .await
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert_eq!(runtime.starts(), 1);
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn address_conflict_retries_with_a_fresh_network_identity_before_commit() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("address-retry");
            let runtime = FakeRuntime::default();
            runtime
                .state
                .address_conflicts_remaining
                .store(1, Ordering::SeqCst);
            let first = NetworkId::from_bytes([0x21; 16]);
            let second = NetworkId::from_bytes([0x22; 16]);
            let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                store,
                DaemonConfig::default(),
                nap_capabilities(Some(true)),
                runtime.clone(),
                SequenceGenerator::new([first, second]),
            );

            let created = operations
                .create_network(DisplayName::new("Retry").unwrap())
                .await
                .unwrap();
            assert_eq!(created, second);
            assert_eq!(runtime.starts(), 2);
            let registry = operations.store.load().unwrap();
            assert!(registry.network(&first).is_none());
            assert_eq!(
                registry.network(&second).unwrap().state,
                MembershipState::Member
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn network_identity_drives_stable_bridge_and_subnet() {
        let network =
            NetworkId::from_bytes([0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let bridge = bridge_name(network).unwrap();
        assert_eq!(bridge.as_str(), "brb-12345678");
        assert!(bridge.as_str().len() <= 15);

        let address = local_star_address(network, &DaemonConfig::default()).unwrap();
        assert_eq!(address.interface, bridge);
        assert_eq!(address.owner, network);
        assert_eq!(address.prefix.prefix_len, 24);
        let IpAddr::V4(ipv4) = address.prefix.address else {
            panic!("P6-001 must allocate IPv4");
        };
        assert_eq!(ipv4.octets()[0..2], [10, 201]);
        assert_eq!(ipv4.octets()[3], 1);
    }
    #[test]
    fn guarded_control_bind_requires_stable_durable_host_membership() {
        use std::net::{SocketAddr, TcpListener, TcpStream};

        #[derive(Clone)]
        struct BindingRuntime {
            host: NetworkId,
            membership_path: std::path::PathBuf,
            replace_membership: Option<NetworkId>,
            binds: Arc<AtomicUsize>,
            bound_address: Arc<Mutex<Option<SocketAddrV4>>>,
        }

        impl StarHostRuntime for BindingRuntime {
            fn bind_guarded_control_listener(
                &self,
                _config: DaemonConfig,
            ) -> NetworkOperationFuture<'_, Option<crate::ObservedControlListener>> {
                Box::pin(async move {
                    self.binds.fetch_add(1, Ordering::SeqCst);
                    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
                    let SocketAddr::V4(endpoint) = socket.local_addr().unwrap() else {
                        panic!("test must bind IPv4");
                    };
                    *self.bound_address.lock().unwrap() = Some(endpoint);
                    if let Some(replacement) = self.replace_membership {
                        let mut registry = MembershipRegistry::default();
                        let mut member = NetworkMembership::new(
                            replacement,
                            DisplayName::new("Replaced host").unwrap(),
                        );
                        member.state = MembershipState::Member;
                        registry.remember_network(member);
                        NetworkMembershipStore::new(&self.membership_path)
                            .save(&registry)
                            .unwrap();
                    }
                    Ok(Some(crate::ObservedControlListener::new(
                        self.host, endpoint, socket,
                    )?))
                })
            }

            fn start_host(
                &self,
                _network: NetworkId,
                _bridge: NetworkInterfaceHandle,
                _address: InterfaceAddress,
            ) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }

            fn stop_host(
                &self,
                _network: NetworkId,
                _bridge: NetworkInterfaceHandle,
                _address: InterfaceAddress,
            ) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }
        }

        futures_lite::future::block_on(async {
            let network = NetworkId::from_bytes([0x91; 16]);
            let other = NetworkId::from_bytes([0x92; 16]);
            for scenario in 0..4 {
                let (root, store) = temp_store("guarded-durable-membership");
                if scenario != 0 {
                    let mut registry = MembershipRegistry::default();
                    let mut member =
                        NetworkMembership::new(network, DisplayName::new("Host").unwrap());
                    member.state = MembershipState::Member;
                    registry.remember_network(member);
                    store.save(&registry).unwrap();
                }
                let binds = Arc::new(AtomicUsize::new(0));
                let bound_address = Arc::new(Mutex::new(None));
                let runtime = BindingRuntime {
                    host: if scenario == 2 { other } else { network },
                    membership_path: root.join("memberships-v1"),
                    replace_membership: if scenario == 3 { Some(other) } else { None },
                    binds: Arc::clone(&binds),
                    bound_address: Arc::clone(&bound_address),
                };
                let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                    store,
                    DaemonConfig::default(),
                    nap_capabilities(Some(true)),
                    runtime,
                    SequenceGenerator::new([]),
                );
                let result = operations.bind_guarded_control_listener().await;
                if scenario == 0 {
                    assert!(result.unwrap().is_none(), "no member cannot bind");
                    assert_eq!(binds.load(Ordering::SeqCst), 0);
                } else if scenario == 1 {
                    let listener = result.unwrap().expect("stable member may bind");
                    assert_eq!(listener.network(), network);
                    let endpoint = bound_address.lock().unwrap().unwrap();
                    assert!(TcpStream::connect(endpoint).is_ok());
                    drop(listener);
                    assert!(TcpStream::connect(endpoint).is_err());
                } else {
                    assert_eq!(result.err().unwrap().kind(), ErrorKind::InvalidState);
                    let endpoint = bound_address.lock().unwrap().unwrap();
                    assert!(
                        TcpStream::connect(endpoint).is_err(),
                        "mismatched or changed membership must close listener"
                    );
                }
                fs::remove_dir_all(root).unwrap();
            }
        });
    }

    #[test]
    fn observed_control_endpoint_rechecks_durable_membership_and_network_identity() {
        #[derive(Clone)]
        struct EndpointRuntime {
            endpoint: SocketAddrV4,
            membership_path: std::path::PathBuf,
            replace_membership: Option<NetworkId>,
            observations: Arc<AtomicUsize>,
        }

        impl StarHostRuntime for EndpointRuntime {
            fn observed_owned_control_endpoint(
                &self,
                _config: DaemonConfig,
            ) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
                Box::pin(async move {
                    self.observations.fetch_add(1, Ordering::SeqCst);
                    if let Some(replacement) = self.replace_membership {
                        let mut registry = MembershipRegistry::default();
                        let mut member = NetworkMembership::new(
                            replacement,
                            DisplayName::new("Changed host").unwrap(),
                        );
                        member.state = MembershipState::Member;
                        registry.remember_network(member);
                        NetworkMembershipStore::new(&self.membership_path)
                            .save(&registry)
                            .unwrap();
                    }
                    Ok(Some(self.endpoint))
                })
            }

            fn start_host(
                &self,
                _network: NetworkId,
                _bridge: NetworkInterfaceHandle,
                _address: InterfaceAddress,
            ) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }

            fn stop_host(
                &self,
                _network: NetworkId,
                _bridge: NetworkInterfaceHandle,
                _address: InterfaceAddress,
            ) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }
        }

        futures_lite::future::block_on(async {
            let network = NetworkId::from_bytes([0x91; 16]);
            let other = NetworkId::from_bytes([0x92; 16]);
            let config = DaemonConfig::default();
            let planned =
                Ipv4StarAddressPlan::for_network(network, config.ipv4_address_pool).unwrap();
            let std::net::IpAddr::V4(ip) = planned.host.address else {
                panic!("NAP host plan must be IPv4");
            };
            let expected = SocketAddrV4::new(ip, crate::CONTROL_TCP_PORT);
            for scenario in 0..4 {
                let (root, store) = temp_store("observed-durable-endpoint");
                if scenario != 0 {
                    let mut registry = MembershipRegistry::default();
                    let mut member =
                        NetworkMembership::new(network, DisplayName::new("Host").unwrap());
                    member.state = MembershipState::Member;
                    registry.remember_network(member);
                    store.save(&registry).unwrap();
                }
                let observations = Arc::new(AtomicUsize::new(0));
                let runtime = EndpointRuntime {
                    endpoint: if scenario == 2 {
                        SocketAddrV4::new(*expected.ip(), expected.port() + 1)
                    } else {
                        expected
                    },
                    membership_path: root.join("memberships-v1"),
                    replace_membership: if scenario == 3 { Some(other) } else { None },
                    observations: Arc::clone(&observations),
                };
                let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                    store,
                    config.clone(),
                    nap_capabilities(Some(true)),
                    runtime,
                    SequenceGenerator::new([]),
                );
                let result = operations.observed_owned_control_endpoint().await;
                match scenario {
                    0 => {
                        assert!(result.unwrap().is_none());
                        assert_eq!(observations.load(Ordering::SeqCst), 0);
                    }
                    1 => {
                        assert_eq!(result.unwrap(), Some(expected));
                        assert_eq!(observations.load(Ordering::SeqCst), 1);
                    }
                    _ => {
                        assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidState);
                        assert_eq!(observations.load(Ordering::SeqCst), 1);
                    }
                }
                fs::remove_dir_all(root).unwrap();
            }
        });
    }

    #[test]
    fn raw_control_bind_cannot_bypass_durable_membership() {
        use std::net::{SocketAddr, TcpListener, TcpStream};

        #[derive(Clone)]
        struct RawRuntime {
            membership_path: std::path::PathBuf,
            replace_membership: Option<NetworkId>,
            binds: Arc<AtomicUsize>,
            bound_address: Arc<Mutex<Option<SocketAddrV4>>>,
        }

        impl StarHostRuntime for RawRuntime {
            fn bind_owned_control_listener(
                &self,
                _config: DaemonConfig,
            ) -> NetworkOperationFuture<'_, Option<TcpListener>> {
                Box::pin(async move {
                    self.binds.fetch_add(1, Ordering::SeqCst);
                    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
                    let SocketAddr::V4(endpoint) = socket.local_addr().unwrap() else {
                        panic!("test must bind IPv4");
                    };
                    *self.bound_address.lock().unwrap() = Some(endpoint);
                    if let Some(replacement) = self.replace_membership {
                        let mut registry = MembershipRegistry::default();
                        let mut member = NetworkMembership::new(
                            replacement,
                            DisplayName::new("Replaced host").unwrap(),
                        );
                        member.state = MembershipState::Member;
                        registry.remember_network(member);
                        NetworkMembershipStore::new(&self.membership_path)
                            .save(&registry)
                            .unwrap();
                    }
                    Ok(Some(socket))
                })
            }

            fn start_host(
                &self,
                _network: NetworkId,
                _bridge: NetworkInterfaceHandle,
                _address: InterfaceAddress,
            ) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }

            fn stop_host(
                &self,
                _network: NetworkId,
                _bridge: NetworkInterfaceHandle,
                _address: InterfaceAddress,
            ) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }
        }

        futures_lite::future::block_on(async {
            let network = NetworkId::from_bytes([0x91; 16]);
            let other = NetworkId::from_bytes([0x92; 16]);
            for scenario in 0..3 {
                let (root, store) = temp_store("raw-control-durable-membership");
                if scenario != 0 {
                    let mut registry = MembershipRegistry::default();
                    let mut member =
                        NetworkMembership::new(network, DisplayName::new("Host").unwrap());
                    member.state = MembershipState::Member;
                    registry.remember_network(member);
                    store.save(&registry).unwrap();
                }
                let binds = Arc::new(AtomicUsize::new(0));
                let bound_address = Arc::new(Mutex::new(None));
                let runtime = RawRuntime {
                    membership_path: root.join("memberships-v1"),
                    replace_membership: if scenario == 2 { Some(other) } else { None },
                    binds: Arc::clone(&binds),
                    bound_address: Arc::clone(&bound_address),
                };
                let operations = SingleStarNetworkOperations::with_runtime_and_generator(
                    store,
                    DaemonConfig::default(),
                    nap_capabilities(Some(true)),
                    runtime,
                    SequenceGenerator::new([]),
                );
                let result = operations.bind_owned_control_listener().await;
                if scenario == 0 {
                    assert!(result.unwrap().is_none());
                    assert_eq!(binds.load(Ordering::SeqCst), 0);
                } else {
                    assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidState);
                    assert_eq!(binds.load(Ordering::SeqCst), 1);
                    let endpoint = bound_address.lock().unwrap().unwrap();
                    assert!(
                        TcpStream::connect(endpoint).is_err(),
                        "raw listener must close after endpoint or membership mismatch"
                    );
                }
                fs::remove_dir_all(root).unwrap();
            }
        });
    }
}

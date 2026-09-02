use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use blueroute_core::{
    CoreError, DaemonConfig, DisplayName, ErrorKind, IpPrefix, MembershipRegistry, MembershipState,
    NetworkId, NetworkMembership, NodeCapabilities,
};
use blueroute_linux::{
    BluetoothBackend, BluezBackend, InterfaceAddress, IpNetworkBackend, NetworkAdvertisement,
    NetworkInterfaceHandle, NetworkManagerBackend, NetworkMembershipStore,
    NetworkStateBackend, PanBackend,
};
use blueroute_protocol::NetworkSummary;

pub type NetworkOperationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

pub trait NetworkOperations: Send + Sync {
    fn create_network(&self, name: DisplayName) -> NetworkOperationFuture<'_, NetworkId>;
    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>>;
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

impl StarHostRuntime for LinuxStarHostRuntime {
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
                    return if active.network == network {
                        Ok(())
                    } else {
                        Err(CoreError::new(
                            ErrorKind::InvalidState,
                            "a different BlueRoute star is already hosted on this daemon",
                        ))
                    };
                }
            }

            let bluez = BluezBackend::connect_system().await?;
            let network_backend = NetworkManagerBackend::connect_system().await?;
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
                    None => return Ok(()),
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

            let network = unique_network_id(&registry, &self.generator)?;
            let bridge = bridge_name(network)?;
            let address = local_star_address(network, &self.config)?;

            self.runtime
                .start_host(network, bridge.clone(), address.clone())
                .await?;

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
    fn create_network(&self, name: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        self.create_network_inner(name)
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

            active
                .bluez
                .stop_discovery(active.adapter.clone())
                .await?;
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

fn discovered_network_summary(network: NetworkId) -> Result<NetworkSummary, CoreError> {
    let id = network.to_string();
    Ok(NetworkSummary {
        id: network,
        name: DisplayName::new(format!("BlueRoute {}", &id[..8]))?,
        member_count: 0,
    })
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
    generator: &impl NetworkIdGenerator,
) -> Result<NetworkId, CoreError> {
    for _ in 0..16 {
        let candidate = generator.generate()?;
        if registry.network(&candidate).is_none() {
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
    let pool = config.ipv4_address_pool;
    let segment_bits = pool.segment_prefix_len - pool.pool_prefix_len;
    let segment_count = 1_u32 << u32::from(segment_bits);
    let bytes = network.as_bytes();
    let selector = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % segment_count;
    let segment_host_bits = 32_u32 - u32::from(pool.segment_prefix_len);
    let segment_offset = selector << segment_host_bits;
    let network_address = u32::from(pool.network)
        .checked_add(segment_offset)
        .ok_or_else(|| CoreError::new(ErrorKind::InvalidInput, "IPv4 pool overflow"))?;
    let local_address = network_address
        .checked_add(1)
        .ok_or_else(|| CoreError::new(ErrorKind::InvalidInput, "IPv4 host address overflow"))?;
    let bridge = bridge_name(network)?;
    Ok(InterfaceAddress {
        interface: bridge,
        prefix: IpPrefix::new(
            IpAddr::V4(Ipv4Addr::from(local_address)),
            pool.segment_prefix_len,
        )?,
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
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use blueroute_core::{CapabilitySource, Sourced};

    use super::*;

    #[derive(Default)]
    struct FakeRuntimeState {
        starts: AtomicUsize,
        stops: AtomicUsize,
        fail_start: AtomicBool,
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
                if self.state.fail_start.load(Ordering::SeqCst) {
                    Err(CoreError::new(ErrorKind::PanFailure, "fake NAP failure"))
                } else {
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
}

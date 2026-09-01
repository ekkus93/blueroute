#![doc = "Linux system adapter boundaries for BlueRoute."]

mod bluez;
mod forwarding;
mod identity;
mod membership_store;
mod networkmanager;
mod pan;
mod secret_store;

pub use bluez::BluezBackend;
pub use identity::{NodeIdentityGenerator, NodeIdentityStore, SystemNodeIdentityGenerator};
pub use membership_store::NetworkMembershipStore;
pub use networkmanager::NetworkManagerBackend;
pub use secret_store::SecretFileStore;

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::SystemTime;

use blueroute_core::{CoreError, IpPrefix, NetworkBackend, NetworkId, NodeCapabilities};

/// Boxed future used by Linux adapter traits without requiring an async-trait dependency.
pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

macro_rules! opaque_handle {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(CoreError::new(
                        blueroute_core::ErrorKind::InvalidInput,
                        concat!(stringify!($name), " cannot be empty"),
                    ));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_handle!(AdapterHandle);
opaque_handle!(PeerHandle);
opaque_handle!(NetworkInterfaceHandle);
opaque_handle!(NetworkConnectionHandle);
opaque_handle!(NetworkDeviceHandle);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothAdapter {
    pub handle: AdapterHandle,
    pub powered: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BluetoothAdapterEvent {
    Added(BluetoothAdapter),
    Removed(AdapterHandle),
    PoweredChanged {
        handle: AdapterHandle,
        powered: bool,
    },
}

/// Pull-based adapter event subscription that remains independent of D-Bus stream types.
pub trait AdapterEventSubscription: Send {
    fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothAdapterEvent>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredPeer {
    pub handle: PeerHandle,
    pub display_name: Option<String>,
    pub paired: bool,
    pub trusted: bool,
}

/// Opaque restore token for a bounded incoming Bluetooth pairing window.
#[derive(Debug, Eq, PartialEq)]
pub struct IncomingPairingWindow {
    pub(crate) adapter: AdapterHandle,
    pub(crate) restore_discoverable: bool,
    pub(crate) restore_pairable: bool,
}

impl IncomingPairingWindow {
    pub fn adapter(&self) -> &AdapterHandle {
        &self.adapter
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BluetoothPeerEvent {
    Added(DiscoveredPeer),
    Changed(DiscoveredPeer),
    Removed(PeerHandle),
}

/// Pull-based peer event subscription independent of BlueZ/D-Bus stream types.
pub trait PeerEventSubscription: Send {
    fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothPeerEvent>>;
}

/// Bluetooth operations that are independent of how PAN profiles are created.
pub trait BluetoothBackend: Send + Sync {
    fn adapters(&self) -> BackendFuture<'_, Vec<BluetoothAdapter>>;
    fn subscribe_adapter_events(&self) -> BackendFuture<'_, Box<dyn AdapterEventSubscription>>;
    fn start_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;
    fn stop_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;
    fn discovered_peers(&self, adapter: AdapterHandle) -> BackendFuture<'_, Vec<DiscoveredPeer>>;
    fn subscribe_peer_events(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, Box<dyn PeerEventSubscription>>;
    fn begin_incoming_pairing(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, IncomingPairingWindow>;
    fn end_incoming_pairing(&self, window: IncomingPairingWindow) -> BackendFuture<'_, ()>;
    fn pair(&self, peer: PeerHandle) -> BackendFuture<'_, ()>;
    fn set_trusted(&self, peer: PeerHandle, trusted: bool) -> BackendFuture<'_, ()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanRole {
    Panu,
    Nap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanAttachment {
    pub role: PanRole,
    pub interface: NetworkInterfaceHandle,
    pub peer: Option<PeerHandle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PanuEvent {
    Lost(PanAttachment),
}

/// Pull-based PANU link event subscription independent of BlueZ/D-Bus stream types.
pub trait PanuEventSubscription: Send {
    fn next_event(&mut self) -> BackendFuture<'_, Option<PanuEvent>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NapEvent {
    ClientAttached(PanAttachment),
    ClientDetached(PanAttachment),
}

/// Pull-based NAP client event subscription independent of BlueZ/D-Bus stream types.
pub trait NapEventSubscription: Send {
    fn next_event(&mut self) -> BackendFuture<'_, Option<NapEvent>>;
}

/// PAN lifecycle boundary. Its implementation may ultimately use BlueZ, NetworkManager, or both.
pub trait PanBackend: Send + Sync {
    fn connect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, PanAttachment>;
    fn disconnect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, ()>;
    fn subscribe_panu_events(
        &self,
        attachment: PanAttachment,
    ) -> BackendFuture<'_, Box<dyn PanuEventSubscription>>;
    fn start_nap(
        &self,
        adapter: AdapterHandle,
        bridge: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, PanAttachment>;
    fn subscribe_nap_events(
        &self,
        adapter: AdapterHandle,
        attachment: PanAttachment,
    ) -> BackendFuture<'_, Box<dyn NapEventSubscription>>;
    fn stop_nap(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkConnection {
    pub handle: NetworkConnectionHandle,
    pub id: String,
    pub uuid: String,
    pub connection_type: String,
    pub interface: Option<NetworkInterfaceHandle>,
    pub owner: Option<NetworkId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkDevice {
    pub handle: NetworkDeviceHandle,
    pub interface: NetworkInterfaceHandle,
    pub managed: bool,
    pub device_type: u32,
    pub state: u32,
    pub active_connection: Option<NetworkConnectionHandle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkStateEvent {
    ConnectionAdded(NetworkConnection),
    ConnectionChanged(NetworkConnection),
    ConnectionRemoved(NetworkConnectionHandle),
    DeviceAdded(NetworkDevice),
    DeviceChanged(NetworkDevice),
    DeviceRemoved(NetworkDeviceHandle),
}

/// Pull-based NetworkManager state observation without exposing D-Bus stream types.
pub trait NetworkStateSubscription: Send {
    fn next_event(&mut self) -> BackendFuture<'_, Option<NetworkStateEvent>>;
}

/// Connection/device lifecycle boundary implemented by the selected Linux network manager.
pub trait NetworkStateBackend: Send + Sync {
    fn network_connections(&self) -> BackendFuture<'_, Vec<NetworkConnection>>;
    fn network_devices(&self) -> BackendFuture<'_, Vec<NetworkDevice>>;
    fn subscribe_network_state(&self) -> BackendFuture<'_, Box<dyn NetworkStateSubscription>>;
    fn ensure_bridge(
        &self,
        owner: NetworkId,
        bridge: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, NetworkConnection>;
    fn remove_owned_interface(
        &self,
        owner: NetworkId,
        interface: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, ()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceAddress {
    pub interface: NetworkInterfaceHandle,
    pub prefix: IpPrefix,
    pub owner: NetworkId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxRoute {
    pub destination: IpPrefix,
    pub via: Option<IpAddr>,
    pub interface: NetworkInterfaceHandle,
    pub metric: u32,
    pub owner: NetworkId,
}

/// Linux IP/network configuration boundary. It is intentionally not NetworkManager-specific.
pub trait IpNetworkBackend: Send + Sync {
    fn addresses(&self) -> BackendFuture<'_, Vec<InterfaceAddress>>;
    fn ensure_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()>;
    fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()>;
    fn routes(&self) -> BackendFuture<'_, Vec<LinuxRoute>>;
    fn ensure_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()>;
    fn remove_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()>;
    fn set_ipv4_forwarding(&self, enabled: bool) -> BackendFuture<'_, ()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCapabilities {
    pub bluez_available: bool,
    pub network_backend: Option<NetworkBackend>,
    pub node: NodeCapabilities,
}

pub trait CapabilityProbe: Send + Sync {
    fn probe(&self) -> BackendFuture<'_, RuntimeCapabilities>;
}

/// Injectable clock for reconciliation/backoff tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// Injectable event boundary so state-machine tests do not require D-Bus or a UI.
pub trait EventSink<E>: Send + Sync {
    fn publish(&self, event: E);
}

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake, Waker};

    use blueroute_core::{CapabilitySource, ErrorKind, Sourced};

    use super::*;

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    fn resolve<T>(mut future: BackendFuture<'_, T>) -> Result<T, CoreError> {
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        match Future::poll(future.as_mut(), &mut context) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("test backend unexpectedly returned a pending future"),
        }
    }

    #[derive(Default)]
    struct FakeBluetooth {
        discovery_started: Mutex<bool>,
    }

    struct EmptyAdapterSubscription;

    impl AdapterEventSubscription for EmptyAdapterSubscription {
        fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothAdapterEvent>> {
            Box::pin(async { Ok(None) })
        }
    }

    struct EmptyPeerSubscription;

    impl PeerEventSubscription for EmptyPeerSubscription {
        fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothPeerEvent>> {
            Box::pin(async { Ok(None) })
        }
    }

    impl BluetoothBackend for FakeBluetooth {
        fn adapters(&self) -> BackendFuture<'_, Vec<BluetoothAdapter>> {
            Box::pin(async {
                Ok(vec![BluetoothAdapter {
                    handle: AdapterHandle::new("fake0")?,
                    powered: true,
                }])
            })
        }

        fn subscribe_adapter_events(&self) -> BackendFuture<'_, Box<dyn AdapterEventSubscription>> {
            Box::pin(async {
                Ok(Box::new(EmptyAdapterSubscription) as Box<dyn AdapterEventSubscription>)
            })
        }

        fn start_discovery(&self, _adapter: AdapterHandle) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                *self.discovery_started.lock().map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::Internal,
                        "fake Bluetooth state lock failed",
                        error.to_string(),
                    )
                })? = true;
                Ok(())
            })
        }

        fn stop_discovery(&self, _adapter: AdapterHandle) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                *self.discovery_started.lock().map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::Internal,
                        "fake Bluetooth state lock failed",
                        error.to_string(),
                    )
                })? = false;
                Ok(())
            })
        }

        fn discovered_peers(
            &self,
            _adapter: AdapterHandle,
        ) -> BackendFuture<'_, Vec<DiscoveredPeer>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn subscribe_peer_events(
            &self,
            _adapter: AdapterHandle,
        ) -> BackendFuture<'_, Box<dyn PeerEventSubscription>> {
            Box::pin(async {
                Ok(Box::new(EmptyPeerSubscription) as Box<dyn PeerEventSubscription>)
            })
        }

        fn begin_incoming_pairing(
            &self,
            adapter: AdapterHandle,
        ) -> BackendFuture<'_, IncomingPairingWindow> {
            Box::pin(async move {
                Ok(IncomingPairingWindow {
                    adapter,
                    restore_discoverable: false,
                    restore_pairable: false,
                })
            })
        }

        fn end_incoming_pairing(&self, _window: IncomingPairingWindow) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn pair(&self, _peer: PeerHandle) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn set_trusted(&self, _peer: PeerHandle, _trusted: bool) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Default)]
    struct FakeIpBackend {
        forwarding: Mutex<bool>,
    }

    impl IpNetworkBackend for FakeIpBackend {
        fn addresses(&self) -> BackendFuture<'_, Vec<InterfaceAddress>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn ensure_address(&self, _address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn remove_address(&self, _address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn routes(&self) -> BackendFuture<'_, Vec<LinuxRoute>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn ensure_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn remove_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn set_ipv4_forwarding(&self, enabled: bool) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                *self.forwarding.lock().map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::Internal,
                        "fake forwarding state lock failed",
                        error.to_string(),
                    )
                })? = enabled;
                Ok(())
            })
        }
    }

    #[test]
    fn fake_bluetooth_backend_exercises_boundary_without_dbus() {
        let backend = FakeBluetooth::default();
        let adapters = resolve(backend.adapters()).unwrap();
        assert_eq!(adapters.len(), 1);
        let mut subscription = resolve(backend.subscribe_adapter_events()).unwrap();
        assert_eq!(resolve(subscription.next_event()).unwrap(), None);
        resolve(backend.start_discovery(adapters[0].handle.clone())).unwrap();
        assert!(*backend.discovery_started.lock().unwrap());
        let mut peers = resolve(backend.subscribe_peer_events(adapters[0].handle.clone())).unwrap();
        assert_eq!(resolve(peers.next_event()).unwrap(), None);
        let window = resolve(backend.begin_incoming_pairing(adapters[0].handle.clone())).unwrap();
        assert_eq!(window.adapter(), &adapters[0].handle);
        resolve(backend.end_incoming_pairing(window)).unwrap();
    }

    #[test]
    fn fake_ip_backend_is_not_networkmanager_specific() {
        let backend = FakeIpBackend::default();
        resolve(backend.set_ipv4_forwarding(true)).unwrap();
        assert!(*backend.forwarding.lock().unwrap());
    }

    #[test]
    fn runtime_capabilities_can_describe_non_networkmanager_backend() {
        let capabilities = RuntimeCapabilities {
            bluez_available: true,
            network_backend: Some(NetworkBackend::SystemdNetworkd),
            node: NodeCapabilities {
                panu: Some(Sourced::new(true, CapabilitySource::Measured)),
                ..NodeCapabilities::default()
            },
        };
        assert_eq!(
            capabilities.network_backend,
            Some(NetworkBackend::SystemdNetworkd)
        );
    }
}

#![doc = "Linux system adapter boundaries for BlueRoute."]

mod identity;
mod membership_store;
mod secret_store;

pub use identity::{NodeIdentityGenerator, NodeIdentityStore, SystemNodeIdentityGenerator};
pub use membership_store::NetworkMembershipStore;
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothAdapter {
    pub handle: AdapterHandle,
    pub powered: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredPeer {
    pub handle: PeerHandle,
    pub display_name: Option<String>,
    pub paired: bool,
    pub trusted: bool,
}

/// Bluetooth operations that are independent of how PAN profiles are created.
pub trait BluetoothBackend: Send + Sync {
    fn adapters(&self) -> BackendFuture<'_, Vec<BluetoothAdapter>>;
    fn start_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;
    fn stop_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;
    fn discovered_peers(&self, adapter: AdapterHandle) -> BackendFuture<'_, Vec<DiscoveredPeer>>;
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

/// PAN lifecycle boundary. Its implementation may ultimately use BlueZ, NetworkManager, or both.
pub trait PanBackend: Send + Sync {
    fn connect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, PanAttachment>;
    fn disconnect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, ()>;
    fn start_nap(&self, adapter: AdapterHandle) -> BackendFuture<'_, PanAttachment>;
    fn stop_nap(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceAddress {
    pub interface: NetworkInterfaceHandle,
    pub prefix: IpPrefix,
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

    impl BluetoothBackend for FakeBluetooth {
        fn adapters(&self) -> BackendFuture<'_, Vec<BluetoothAdapter>> {
            Box::pin(async {
                Ok(vec![BluetoothAdapter {
                    handle: AdapterHandle::new("fake0")?,
                    powered: true,
                }])
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
        resolve(backend.start_discovery(adapters[0].handle.clone())).unwrap();
        assert!(*backend.discovery_started.lock().unwrap());
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

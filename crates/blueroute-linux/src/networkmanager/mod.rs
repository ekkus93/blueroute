mod addresses;
mod dbus;
mod inventory;
mod ownership;
mod profile;
mod routes;

use self::addresses::{ensure_address, owned_addresses, remove_address};
use self::dbus::{ensure_networkmanager_available, network_error, network_manager_proxy};
use self::inventory::{
    NetworkManagerSubscription, connection_snapshot, device_snapshot, list_connections,
    list_devices, push_event,
};
use self::profile::{ensure_bridge, remove_owned_interface};
use self::routes::{ensure_route, owned_routes, remove_route};
use crate::{
    BackendFuture, InterfaceAddress, IpNetworkBackend, LinuxRoute, NetworkConnection,
    NetworkDevice, NetworkInterfaceHandle, NetworkStateBackend, NetworkStateEvent,
    NetworkStateSubscription,
};
use blueroute_core::{CoreError, ErrorKind, NetworkId};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use zbus::Connection;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::zvariant::OwnedValue;

const NM_SERVICE: &str = "org.freedesktop.NetworkManager";

const NM_PATH: &str = "/org/freedesktop/NetworkManager";

const NM_INTERFACE: &str = "org.freedesktop.NetworkManager";

const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";

const SETTINGS_INTERFACE: &str = "org.freedesktop.NetworkManager.Settings";

const SETTINGS_CONNECTION_INTERFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";

const DEVICE_INTERFACE: &str = "org.freedesktop.NetworkManager.Device";

const ACTIVE_CONNECTION_INTERFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";

const CONNECTION_SETTING: &str = "connection";

const USER_SETTING: &str = "user";

const IPV4_SETTING: &str = "ipv4";

const IPV6_SETTING: &str = "ipv6";

const USER_DATA_PROPERTY: &str = "data";

const OWNER_KEY: &str = "org.blueroute.owner";

const KIND_KEY: &str = "org.blueroute.kind";

const SCHEMA_KEY: &str = "org.blueroute.schema";

const OWNERSHIP_SCHEMA: &str = "1";

const KIND_BRIDGE: &str = "bridge";

const KIND_INTERFACE: &str = "interface";

const DEVICE_TYPE_BRIDGE: u32 = 13;

const ADD_CONNECTION2_TO_DISK: u32 = 0x1;

const ADD_CONNECTION2_BLOCK_AUTOCONNECT: u32 = 0x20;

const UPDATE2_TO_DISK: u32 = 0x1;

const RECONCILE_INTERVAL: Duration = Duration::from_millis(250);

const APPLY_TIMEOUT: Duration = Duration::from_secs(8);

const EVENT_QUEUE_CAPACITY: usize = 256;

type NmSettings = HashMap<String, HashMap<String, OwnedValue>>;

/// Direct system-D-Bus NetworkManager backend. It never invokes or parses `nmcli`.
#[derive(Clone)]
pub struct NetworkManagerBackend {
    connection: Connection,
}

impl NetworkManagerBackend {
    /// Connect to the system bus and verify that NetworkManager currently owns its D-Bus name.
    pub async fn connect_system() -> Result<Self, CoreError> {
        let connection = Connection::system().await.map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to connect to the system D-Bus for NetworkManager",
                error,
            )
        })?;
        ensure_networkmanager_available(&connection).await?;
        Ok(Self { connection })
    }

    /// The unique bus name of the running NetworkManager instance, or `None` while it is gone.
    /// It changes whenever NetworkManager restarts.
    pub async fn service_owner(&self) -> Result<Option<String>, CoreError> {
        let proxy = DBusProxy::new(&self.connection).await.map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to access the system D-Bus broker",
                error,
            )
        })?;
        let name = BusName::try_from(NM_SERVICE).map_err(|error| {
            network_error(
                ErrorKind::Internal,
                "invalid built-in NetworkManager D-Bus service name",
                error,
            )
        })?;
        if !proxy.name_has_owner(name.clone()).await.map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to query NetworkManager availability",
                error,
            )
        })? {
            return Ok(None);
        }
        proxy
            .get_name_owner(name)
            .await
            .map(|owner| Some(owner.as_str().to_owned()))
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to identify the current NetworkManager instance",
                    error,
                )
            })
    }

    /// Return the NetworkManager version exposed by the live system service.
    pub async fn version(&self) -> Result<String, CoreError> {
        network_manager_proxy(&self.connection)
            .await?
            .get_property("Version")
            .await
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to read the NetworkManager version",
                    error,
                )
            })
    }
}

impl NetworkStateBackend for NetworkManagerBackend {
    fn network_connections(&self) -> BackendFuture<'_, Vec<NetworkConnection>> {
        Box::pin(async move { list_connections(&self.connection).await })
    }

    fn network_devices(&self) -> BackendFuture<'_, Vec<NetworkDevice>> {
        Box::pin(async move { list_devices(&self.connection).await })
    }

    fn subscribe_network_state(&self) -> BackendFuture<'_, Box<dyn NetworkStateSubscription>> {
        Box::pin(async move {
            let connections = connection_snapshot(&self.connection).await?;
            let devices = device_snapshot(&self.connection).await?;
            let mut pending = VecDeque::new();
            for value in connections.values() {
                push_event(
                    &mut pending,
                    NetworkStateEvent::ConnectionAdded(value.clone()),
                )?;
            }
            for value in devices.values() {
                push_event(&mut pending, NetworkStateEvent::DeviceAdded(value.clone()))?;
            }
            Ok(Box::new(NetworkManagerSubscription {
                connection: self.connection.clone(),
                connections,
                devices,
                pending,
            }) as Box<dyn NetworkStateSubscription>)
        })
    }

    fn ensure_bridge(
        &self,
        owner: NetworkId,
        bridge: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, NetworkConnection> {
        Box::pin(async move { ensure_bridge(&self.connection, owner, &bridge).await })
    }

    fn remove_owned_interface(
        &self,
        owner: NetworkId,
        interface: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, ()> {
        Box::pin(async move { remove_owned_interface(&self.connection, owner, &interface).await })
    }
}

impl IpNetworkBackend for NetworkManagerBackend {
    fn addresses(&self) -> BackendFuture<'_, Vec<InterfaceAddress>> {
        Box::pin(async move { owned_addresses(&self.connection).await })
    }

    fn ensure_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
        Box::pin(async move { ensure_address(&self.connection, &address).await })
    }

    fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
        Box::pin(async move { remove_address(&self.connection, &address).await })
    }

    fn routes(&self) -> BackendFuture<'_, Vec<LinuxRoute>> {
        Box::pin(async move { owned_routes(&self.connection).await })
    }

    fn ensure_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()> {
        Box::pin(async move { ensure_route(&self.connection, &route).await })
    }

    fn remove_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()> {
        Box::pin(async move { remove_route(&self.connection, &route).await })
    }

    fn set_ipv4_forwarding(&self, enabled: bool) -> BackendFuture<'_, ()> {
        Box::pin(async move { crate::forwarding::set_ipv4_forwarding(enabled) })
    }
}

#[cfg(test)]
mod test_support {

    use super::*;

    pub(super) fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    pub(super) fn interface(value: &str) -> NetworkInterfaceHandle {
        NetworkInterfaceHandle::new(value).unwrap()
    }
}

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::IpAddr;
use std::str::FromStr;
use std::time::{Duration, Instant};

use async_io::Timer;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Str, Value};
use zbus::{Connection, Proxy};

use blueroute_core::{CoreError, ErrorKind, IpPrefix, NetworkId};

use crate::{
    BackendFuture, InterfaceAddress, IpNetworkBackend, LinuxRoute, NetworkConnection,
    NetworkConnectionHandle, NetworkDevice, NetworkDeviceHandle, NetworkInterfaceHandle,
    NetworkStateBackend, NetworkStateEvent, NetworkStateSubscription,
};

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
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "NetworkManager route management is implemented by P4-008, not P4-007",
            ))
        })
    }

    fn ensure_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "NetworkManager route management is implemented by P4-008, not P4-007",
            ))
        })
    }

    fn remove_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "NetworkManager route management is implemented by P4-008, not P4-007",
            ))
        })
    }

    fn set_ipv4_forwarding(&self, _enabled: bool) -> BackendFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "IPv4 forwarding is implemented by P4-009, not P4-007",
            ))
        })
    }
}

struct NetworkManagerSubscription {
    connection: Connection,
    connections: BTreeMap<NetworkConnectionHandle, NetworkConnection>,
    devices: BTreeMap<NetworkDeviceHandle, NetworkDevice>,
    pending: VecDeque<NetworkStateEvent>,
}

impl NetworkStateSubscription for NetworkManagerSubscription {
    fn next_event(&mut self) -> BackendFuture<'_, Option<NetworkStateEvent>> {
        Box::pin(async move {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            loop {
                Timer::after(RECONCILE_INTERVAL).await;
                let current_connections = connection_snapshot(&self.connection).await?;
                let current_devices = device_snapshot(&self.connection).await?;
                queue_connection_changes(
                    &self.connections,
                    &current_connections,
                    &mut self.pending,
                )?;
                queue_device_changes(&self.devices, &current_devices, &mut self.pending)?;
                self.connections = current_connections;
                self.devices = current_devices;
                if let Some(event) = self.pending.pop_front() {
                    return Ok(Some(event));
                }
            }
        })
    }
}

async fn ensure_networkmanager_available(connection: &Connection) -> Result<(), CoreError> {
    let proxy = DBusProxy::new(connection).await.map_err(|error| {
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
    if !proxy.name_has_owner(name).await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to query NetworkManager availability",
            error,
        )
    })? {
        return Err(CoreError::new(
            ErrorKind::NetworkBackendUnavailable,
            "NetworkManager is not available on the system D-Bus",
        ));
    }
    Ok(())
}

async fn network_manager_proxy(connection: &Connection) -> Result<Proxy<'_>, CoreError> {
    Proxy::new(connection, NM_SERVICE, NM_PATH, NM_INTERFACE)
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to create the NetworkManager D-Bus proxy",
                error,
            )
        })
}

async fn settings_proxy(connection: &Connection) -> Result<Proxy<'_>, CoreError> {
    Proxy::new(connection, NM_SERVICE, SETTINGS_PATH, SETTINGS_INTERFACE)
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to create the NetworkManager settings proxy",
                error,
            )
        })
}

async fn settings_connection_proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
) -> Result<Proxy<'a>, CoreError> {
    Proxy::new(connection, NM_SERVICE, path, SETTINGS_CONNECTION_INTERFACE)
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to access a NetworkManager connection profile",
                error,
            )
        })
}

async fn device_proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
) -> Result<Proxy<'a>, CoreError> {
    Proxy::new(connection, NM_SERVICE, path, DEVICE_INTERFACE)
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to access a NetworkManager device",
                error,
            )
        })
}

async fn active_connection_proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
) -> Result<Proxy<'a>, CoreError> {
    Proxy::new(connection, NM_SERVICE, path, ACTIVE_CONNECTION_INTERFACE)
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to access a NetworkManager active connection",
                error,
            )
        })
}

async fn list_connections(connection: &Connection) -> Result<Vec<NetworkConnection>, CoreError> {
    ensure_networkmanager_available(connection).await?;
    let paths: Vec<OwnedObjectPath> = settings_proxy(connection)
        .await?
        .call("ListConnections", &())
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to enumerate NetworkManager connection profiles",
                error,
            )
        })?;
    let mut result = Vec::with_capacity(paths.len());
    for path in paths {
        let path = path.to_string();
        let settings = get_settings(connection, &path).await?;
        result.push(connection_from_settings(&path, &settings)?);
    }
    result.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(result)
}

async fn list_devices(connection: &Connection) -> Result<Vec<NetworkDevice>, CoreError> {
    ensure_networkmanager_available(connection).await?;
    let paths: Vec<OwnedObjectPath> = network_manager_proxy(connection)
        .await?
        .call("GetDevices", &())
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to enumerate NetworkManager devices",
                error,
            )
        })?;
    let mut result = Vec::with_capacity(paths.len());
    for path in paths {
        result.push(read_device(connection, path.as_str()).await?);
    }
    result.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(result)
}

async fn get_settings(connection: &Connection, path: &str) -> Result<NmSettings, CoreError> {
    settings_connection_proxy(connection, path)
        .await?
        .call("GetSettings", &())
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to read a NetworkManager connection profile",
                error,
            )
        })
}

fn connection_from_settings(
    path: &str,
    settings: &NmSettings,
) -> Result<NetworkConnection, CoreError> {
    let connection = settings.get(CONNECTION_SETTING).ok_or_else(|| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned a connection profile without connection settings",
            format!("profile path: {path}"),
        )
    })?;
    let id = setting_string(connection, "id")?.to_owned();
    let uuid = setting_string(connection, "uuid")?.to_owned();
    let connection_type = setting_string(connection, "type")?.to_owned();
    let interface = optional_setting_string(connection, "interface-name")?
        .map(NetworkInterfaceHandle::new)
        .transpose()?;
    let owner = profile_owner(settings)?;
    Ok(NetworkConnection {
        handle: NetworkConnectionHandle::new(path)?,
        id,
        uuid,
        connection_type,
        interface,
        owner,
    })
}

async fn read_device(connection: &Connection, path: &str) -> Result<NetworkDevice, CoreError> {
    let proxy = device_proxy(connection, path).await?;
    let interface: String = proxy.get_property("Interface").await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to read a NetworkManager device interface",
            error,
        )
    })?;
    let managed: bool = proxy.get_property("Managed").await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to read NetworkManager device ownership state",
            error,
        )
    })?;
    let device_type: u32 = proxy.get_property("DeviceType").await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to read a NetworkManager device type",
            error,
        )
    })?;
    let state: u32 = proxy.get_property("State").await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to read a NetworkManager device state",
            error,
        )
    })?;
    let active_path: OwnedObjectPath =
        proxy
            .get_property("ActiveConnection")
            .await
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to read a NetworkManager device active connection",
                    error,
                )
            })?;
    let active_connection = if active_path.as_str() == "/" {
        None
    } else {
        let active_proxy = active_connection_proxy(connection, active_path.as_str()).await?;
        let settings_path: OwnedObjectPath = active_proxy
            .get_property("Connection")
            .await
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to resolve a NetworkManager active connection",
                    error,
                )
            })?;
        Some(NetworkConnectionHandle::new(settings_path.to_string())?)
    };
    Ok(NetworkDevice {
        handle: NetworkDeviceHandle::new(path)?,
        interface: NetworkInterfaceHandle::new(interface)?,
        managed,
        device_type,
        state,
        active_connection,
    })
}

async fn ensure_bridge(
    connection: &Connection,
    owner: NetworkId,
    bridge: &NetworkInterfaceHandle,
) -> Result<NetworkConnection, CoreError> {
    validate_interface_name(bridge)?;
    let profiles = matching_profiles(connection, owner, bridge).await?;
    let path = match exactly_one_owned_profile(profiles, owner, bridge)? {
        Some(profile) => {
            if profile.connection.connection_type != KIND_BRIDGE {
                return Err(CoreError::with_diagnostic(
                    ErrorKind::InvalidState,
                    "BlueRoute owns a non-bridge profile for the requested bridge interface",
                    format!(
                        "interface={} profile-type={}",
                        bridge.as_str(),
                        profile.connection.connection_type
                    ),
                ));
            }
            profile.path
        }
        None => {
            reject_foreign_interface_claim(connection, owner, bridge).await?;
            add_owned_profile(connection, owner, bridge, KIND_BRIDGE).await?
        }
    };
    activate_profile(connection, &path, bridge).await?;
    wait_for_profile_active(connection, &path, bridge).await?;
    let settings = get_settings(connection, &path).await?;
    connection_from_settings(&path, &settings)
}

async fn owned_addresses(connection: &Connection) -> Result<Vec<InterfaceAddress>, CoreError> {
    let paths: Vec<OwnedObjectPath> = settings_proxy(connection)
        .await?
        .call("ListConnections", &())
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to enumerate NetworkManager connection profiles",
                error,
            )
        })?;
    let mut addresses = Vec::new();
    for path in paths {
        let settings = get_settings(connection, path.as_str()).await?;
        let Some(owner) = profile_owner(&settings)? else {
            continue;
        };
        let connection = connection_from_settings(path.as_str(), &settings)?;
        let Some(interface) = connection.interface else {
            continue;
        };
        for prefix in address_prefixes(&settings)? {
            addresses.push(InterfaceAddress {
                interface: interface.clone(),
                prefix,
                owner,
            });
        }
    }
    addresses.sort_by(|left, right| {
        left.interface
            .cmp(&right.interface)
            .then_with(|| left.owner.cmp(&right.owner))
            .then_with(|| left.prefix.cmp(&right.prefix))
    });
    Ok(addresses)
}

async fn ensure_address(
    connection: &Connection,
    address: &InterfaceAddress,
) -> Result<(), CoreError> {
    validate_interface_name(&address.interface)?;
    let profiles = matching_profiles(connection, address.owner, &address.interface).await?;
    let path = match exactly_one_owned_profile(profiles, address.owner, &address.interface)? {
        Some(profile) => profile.path,
        None => {
            let device = device_by_interface(connection, &address.interface)
                .await?
                .ok_or_else(|| {
                    CoreError::with_diagnostic(
                        ErrorKind::InvalidInput,
                        "cannot configure an address on an interface NetworkManager does not know",
                        format!("interface={}", address.interface.as_str()),
                    )
                })?;
            reject_foreign_active_connection(connection, &device, address.owner).await?;
            let kind = if device.device_type == DEVICE_TYPE_BRIDGE {
                KIND_BRIDGE
            } else {
                KIND_INTERFACE
            };
            add_owned_profile(connection, address.owner, &address.interface, kind).await?
        }
    };

    let mut settings = get_settings(connection, &path).await?;
    if profile_owner(&settings)? != Some(address.owner) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "refusing to modify a NetworkManager profile not owned by the requested BlueRoute network",
        ));
    }
    let mut prefixes = address_prefixes(&settings)?;
    if !prefixes.contains(&address.prefix) {
        prefixes.push(address.prefix);
        prefixes.sort();
        set_address_prefixes(&mut settings, &prefixes)?;
        update_profile(connection, &path, &settings).await?;
    }
    activate_profile(connection, &path, &address.interface).await?;
    reapply_if_active(connection, &path, &address.interface, &settings).await?;
    wait_for_address(connection, address, true).await
}

async fn remove_address(
    connection: &Connection,
    address: &InterfaceAddress,
) -> Result<(), CoreError> {
    let profiles = matching_profiles(connection, address.owner, &address.interface).await?;
    let Some(profile) = exactly_one_owned_profile(profiles, address.owner, &address.interface)?
    else {
        return Ok(());
    };
    let mut settings = profile.settings;
    if profile_owner(&settings)? != Some(address.owner) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "refusing to modify a NetworkManager profile not owned by the requested BlueRoute network",
        ));
    }
    let mut prefixes = address_prefixes(&settings)?;
    let before = prefixes.len();
    prefixes.retain(|prefix| prefix != &address.prefix);
    if prefixes.len() == before {
        return Ok(());
    }
    set_address_prefixes(&mut settings, &prefixes)?;
    update_profile(connection, &profile.path, &settings).await?;
    reapply_if_active(connection, &profile.path, &address.interface, &settings).await?;
    wait_for_address(connection, address, false).await
}

async fn remove_owned_interface(
    connection: &Connection,
    owner: NetworkId,
    interface: &NetworkInterfaceHandle,
) -> Result<(), CoreError> {
    let profiles = matching_profiles(connection, owner, interface).await?;
    let Some(profile) = exactly_one_owned_profile(profiles, owner, interface)? else {
        return Ok(());
    };
    if profile_owner(&profile.settings)? != Some(owner) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "refusing to remove a NetworkManager profile not owned by the requested BlueRoute network",
        ));
    }
    if let Some(device) = device_by_interface(connection, interface).await?
        && device.active_connection.as_ref() == Some(&profile.connection.handle)
    {
        deactivate_device_connection(connection, &device).await?;
    }
    settings_connection_proxy(connection, &profile.path)
        .await?
        .call::<_, _, ()>("Delete", &())
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to remove a BlueRoute-owned NetworkManager profile",
                error,
            )
        })?;
    wait_for_profile_absent(connection, &profile.path).await
}

struct OwnedProfile {
    path: String,
    connection: NetworkConnection,
    settings: NmSettings,
}

async fn matching_profiles(
    connection: &Connection,
    owner: NetworkId,
    interface: &NetworkInterfaceHandle,
) -> Result<Vec<OwnedProfile>, CoreError> {
    let paths: Vec<OwnedObjectPath> = settings_proxy(connection)
        .await?
        .call("ListConnections", &())
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to enumerate NetworkManager connection profiles",
                error,
            )
        })?;
    let mut profiles = Vec::new();
    for path in paths {
        let path = path.to_string();
        let settings = get_settings(connection, &path).await?;
        if profile_owner(&settings)? != Some(owner) {
            continue;
        }
        let connection_value = connection_from_settings(&path, &settings)?;
        if connection_value.interface.as_ref() != Some(interface) {
            continue;
        }
        profiles.push(OwnedProfile {
            path,
            connection: connection_value,
            settings,
        });
    }
    Ok(profiles)
}

fn exactly_one_owned_profile(
    mut profiles: Vec<OwnedProfile>,
    owner: NetworkId,
    interface: &NetworkInterfaceHandle,
) -> Result<Option<OwnedProfile>, CoreError> {
    match profiles.len() {
        0 => Ok(None),
        1 => Ok(profiles.pop()),
        count => Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "multiple BlueRoute-owned NetworkManager profiles target the same interface",
            format!(
                "owner={owner} interface={} count={count}",
                interface.as_str()
            ),
        )),
    }
}

async fn reject_foreign_interface_claim(
    connection: &Connection,
    owner: NetworkId,
    interface: &NetworkInterfaceHandle,
) -> Result<(), CoreError> {
    for profile in list_connections(connection).await? {
        if profile.interface.as_ref() == Some(interface) && profile.owner != Some(owner) {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "refusing to create a BlueRoute profile for an interface claimed by another NetworkManager profile",
                format!(
                    "owner={owner} interface={} conflicting-profile={} conflicting-owner={:?}",
                    interface.as_str(),
                    profile.id,
                    profile.owner
                ),
            ));
        }
    }
    if let Some(device) = device_by_interface(connection, interface).await?
        && device.active_connection.is_some()
    {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "refusing to take over an already-active foreign NetworkManager interface",
            format!("interface={}", interface.as_str()),
        ));
    }
    Ok(())
}

async fn reject_foreign_active_connection(
    connection: &Connection,
    device: &NetworkDevice,
    owner: NetworkId,
) -> Result<(), CoreError> {
    let Some(active) = device.active_connection.as_ref() else {
        return Ok(());
    };
    let settings = get_settings(connection, active.as_str()).await?;
    if profile_owner(&settings)? == Some(owner) {
        return Ok(());
    }
    Err(CoreError::with_diagnostic(
        ErrorKind::InvalidState,
        "refusing to replace an active NetworkManager connection not owned by this BlueRoute network",
        format!(
            "interface={} active-profile={}",
            device.interface.as_str(),
            active.as_str()
        ),
    ))
}

async fn add_owned_profile(
    connection: &Connection,
    owner: NetworkId,
    interface: &NetworkInterfaceHandle,
    kind: &str,
) -> Result<String, CoreError> {
    let settings = base_owned_settings(owner, interface, kind)?;
    let args: HashMap<String, OwnedValue> = HashMap::new();
    let flags = ADD_CONNECTION2_TO_DISK | ADD_CONNECTION2_BLOCK_AUTOCONNECT;
    let (path, _result): (OwnedObjectPath, HashMap<String, OwnedValue>) =
        settings_proxy(connection)
            .await?
            .call("AddConnection2", &(settings, flags, args))
            .await
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to create a BlueRoute-owned NetworkManager profile",
                    error,
                )
            })?;
    Ok(path.to_string())
}

fn base_owned_settings(
    owner: NetworkId,
    interface: &NetworkInterfaceHandle,
    kind: &str,
) -> Result<NmSettings, CoreError> {
    let connection_type = match kind {
        KIND_BRIDGE => KIND_BRIDGE,
        KIND_INTERFACE => "generic",
        other => {
            return Err(CoreError::with_diagnostic(
                ErrorKind::Internal,
                "invalid internal NetworkManager profile kind",
                other,
            ));
        }
    };
    let mut connection_setting = HashMap::new();
    connection_setting.insert(
        "id".to_owned(),
        owned_string(format!("blueroute-{kind}-{}", short_owner(owner))),
    );
    connection_setting.insert(
        "uuid".to_owned(),
        owned_string(profile_uuid(owner, interface, kind)),
    );
    connection_setting.insert("type".to_owned(), owned_string(connection_type));
    connection_setting.insert(
        "interface-name".to_owned(),
        owned_string(interface.as_str()),
    );
    connection_setting.insert("autoconnect".to_owned(), OwnedValue::from(false));

    let mut user_data = HashMap::<String, String>::new();
    user_data.insert(OWNER_KEY.to_owned(), owner.to_string());
    user_data.insert(KIND_KEY.to_owned(), kind.to_owned());
    user_data.insert(SCHEMA_KEY.to_owned(), OWNERSHIP_SCHEMA.to_owned());
    let mut user_setting = HashMap::new();
    user_setting.insert(USER_DATA_PROPERTY.to_owned(), OwnedValue::from(user_data));

    let mut ipv4 = HashMap::new();
    ipv4.insert("method".to_owned(), owned_string("disabled"));
    ipv4.insert("never-default".to_owned(), OwnedValue::from(true));
    let mut ipv6 = HashMap::new();
    ipv6.insert("method".to_owned(), owned_string("disabled"));
    ipv6.insert("never-default".to_owned(), OwnedValue::from(true));

    let mut settings = HashMap::new();
    settings.insert(CONNECTION_SETTING.to_owned(), connection_setting);
    settings.insert(USER_SETTING.to_owned(), user_setting);
    settings.insert(IPV4_SETTING.to_owned(), ipv4);
    settings.insert(IPV6_SETTING.to_owned(), ipv6);
    if kind == KIND_BRIDGE {
        settings.insert(KIND_BRIDGE.to_owned(), HashMap::new());
    }
    Ok(settings)
}

async fn update_profile(
    connection: &Connection,
    path: &str,
    settings: &NmSettings,
) -> Result<(), CoreError> {
    let args: HashMap<String, OwnedValue> = HashMap::new();
    let _result: HashMap<String, OwnedValue> = settings_connection_proxy(connection, path)
        .await?
        .call("Update2", &(settings, UPDATE2_TO_DISK, args))
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to update a BlueRoute-owned NetworkManager profile",
                error,
            )
        })?;
    Ok(())
}

async fn activate_profile(
    connection: &Connection,
    path: &str,
    interface: &NetworkInterfaceHandle,
) -> Result<(), CoreError> {
    if let Some(device) = device_by_interface(connection, interface).await? {
        if device
            .active_connection
            .as_ref()
            .map(NetworkConnectionHandle::as_str)
            == Some(path)
        {
            return Ok(());
        }
        if device.active_connection.is_some() {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "refusing to replace an active NetworkManager connection while activating BlueRoute state",
                format!("interface={}", interface.as_str()),
            ));
        }
        let connection_path = object_path(path)?;
        let device_path = object_path(device.handle.as_str())?;
        let root = object_path("/")?;
        let _active: OwnedObjectPath = network_manager_proxy(connection)
            .await?
            .call("ActivateConnection", &(connection_path, device_path, root))
            .await
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to activate a BlueRoute NetworkManager profile",
                    error,
                )
            })?;
        return Ok(());
    }

    let connection_path = object_path(path)?;
    let root = object_path("/")?;
    let _active: OwnedObjectPath = network_manager_proxy(connection)
        .await?
        .call("ActivateConnection", &(connection_path, root.clone(), root))
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to activate a BlueRoute virtual NetworkManager profile",
                error,
            )
        })?;
    Ok(())
}

async fn reapply_if_active(
    connection: &Connection,
    profile_path: &str,
    interface: &NetworkInterfaceHandle,
    settings: &NmSettings,
) -> Result<(), CoreError> {
    let Some(device) = device_by_interface(connection, interface).await? else {
        return Ok(());
    };
    if device
        .active_connection
        .as_ref()
        .map(NetworkConnectionHandle::as_str)
        != Some(profile_path)
    {
        return Ok(());
    }
    device_proxy(connection, device.handle.as_str())
        .await?
        .call::<_, _, ()>("Reapply", &(settings, 0_u64, 0_u32))
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to apply updated BlueRoute addressing to an active NetworkManager device",
                error,
            )
        })
}

async fn deactivate_device_connection(
    connection: &Connection,
    device: &NetworkDevice,
) -> Result<(), CoreError> {
    let proxy = device_proxy(connection, device.handle.as_str()).await?;
    let active_path: OwnedObjectPath =
        proxy
            .get_property("ActiveConnection")
            .await
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to resolve a BlueRoute active connection for cleanup",
                    error,
                )
            })?;
    if active_path.as_str() == "/" {
        return Ok(());
    }
    network_manager_proxy(connection)
        .await?
        .call::<_, _, ()>("DeactivateConnection", &(active_path,))
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to deactivate a BlueRoute-owned NetworkManager connection",
                error,
            )
        })
}

async fn device_by_interface(
    connection: &Connection,
    interface: &NetworkInterfaceHandle,
) -> Result<Option<NetworkDevice>, CoreError> {
    for device in list_devices(connection).await? {
        if device.interface == *interface {
            return Ok(Some(device));
        }
    }
    Ok(None)
}

async fn wait_for_profile_active(
    connection: &Connection,
    profile_path: &str,
    interface: &NetworkInterfaceHandle,
) -> Result<(), CoreError> {
    let deadline = Instant::now() + APPLY_TIMEOUT;
    loop {
        if let Some(device) = device_by_interface(connection, interface).await?
            && device
                .active_connection
                .as_ref()
                .map(NetworkConnectionHandle::as_str)
                == Some(profile_path)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                "NetworkManager did not activate the BlueRoute profile before the timeout",
                format!("interface={} profile={profile_path}", interface.as_str()),
            ));
        }
        Timer::after(RECONCILE_INTERVAL).await;
    }
}

async fn wait_for_profile_absent(connection: &Connection, path: &str) -> Result<(), CoreError> {
    let deadline = Instant::now() + APPLY_TIMEOUT;
    loop {
        let profiles = list_connections(connection).await?;
        if profiles
            .iter()
            .all(|profile| profile.handle.as_str() != path)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                "NetworkManager did not remove the BlueRoute profile before the timeout",
                path,
            ));
        }
        Timer::after(RECONCILE_INTERVAL).await;
    }
}

async fn wait_for_address(
    connection: &Connection,
    address: &InterfaceAddress,
    present: bool,
) -> Result<(), CoreError> {
    let deadline = Instant::now() + APPLY_TIMEOUT;
    loop {
        let current = owned_addresses(connection).await?;
        let found = current.contains(address);
        if found == present {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                if present {
                    "NetworkManager did not record the BlueRoute address before the timeout"
                } else {
                    "NetworkManager did not remove the BlueRoute address before the timeout"
                },
                format!(
                    "owner={} interface={} prefix={:?}",
                    address.owner,
                    address.interface.as_str(),
                    address.prefix
                ),
            ));
        }
        Timer::after(RECONCILE_INTERVAL).await;
    }
}

fn profile_owner(settings: &NmSettings) -> Result<Option<NetworkId>, CoreError> {
    let Some(user) = settings.get(USER_SETTING) else {
        return Ok(None);
    };
    let Some(value) = user.get(USER_DATA_PROPERTY) else {
        return Ok(None);
    };
    let data = HashMap::<String, String>::try_from(value.clone()).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned malformed user connection metadata",
            error.to_string(),
        )
    })?;
    let Some(owner) = data.get(OWNER_KEY) else {
        return Ok(None);
    };
    if data.get(SCHEMA_KEY).map(String::as_str) != Some(OWNERSHIP_SCHEMA) {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "NetworkManager profile has an unsupported BlueRoute ownership schema",
            format!("owner={owner} schema={:?}", data.get(SCHEMA_KEY)),
        ));
    }
    let Some(kind) = data.get(KIND_KEY) else {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "NetworkManager profile has incomplete BlueRoute ownership metadata",
            format!("owner={owner} missing={KIND_KEY}"),
        ));
    };
    if !matches!(kind.as_str(), KIND_BRIDGE | KIND_INTERFACE) {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "NetworkManager profile has an unsupported BlueRoute ownership kind",
            format!("owner={owner} kind={kind}"),
        ));
    }
    NetworkId::from_str(owner).map(Some).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "NetworkManager profile has an invalid BlueRoute owner identifier",
            error.to_string(),
        )
    })
}

fn address_prefixes(settings: &NmSettings) -> Result<Vec<IpPrefix>, CoreError> {
    let mut result = Vec::new();
    for family in [IPV4_SETTING, IPV6_SETTING] {
        let Some(group) = settings.get(family) else {
            continue;
        };
        let Some(value) = group.get("address-data") else {
            continue;
        };
        let entries =
            Vec::<HashMap<String, OwnedValue>>::try_from(value.clone()).map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::ProtocolError,
                    "NetworkManager returned malformed address-data",
                    error.to_string(),
                )
            })?;
        for entry in entries {
            let address = setting_string(&entry, "address")?;
            let prefix = u32::try_from(
                entry
                    .get("prefix")
                    .ok_or_else(|| {
                        CoreError::new(
                            ErrorKind::ProtocolError,
                            "NetworkManager address-data entry is missing a prefix",
                        )
                    })?
                    .clone(),
            )
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::ProtocolError,
                    "NetworkManager returned an invalid address prefix",
                    error.to_string(),
                )
            })?;
            let address = IpAddr::from_str(address).map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::ProtocolError,
                    "NetworkManager returned an invalid IP address",
                    error.to_string(),
                )
            })?;
            let prefix = u8::try_from(prefix).map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::ProtocolError,
                    "NetworkManager returned an out-of-range IP prefix length",
                    error.to_string(),
                )
            })?;
            result.push(IpPrefix::new(address, prefix)?);
        }
    }
    result.sort();
    result.dedup();
    Ok(result)
}

fn set_address_prefixes(settings: &mut NmSettings, prefixes: &[IpPrefix]) -> Result<(), CoreError> {
    for (family_name, ipv4) in [(IPV4_SETTING, true), (IPV6_SETTING, false)] {
        let family_prefixes: Vec<IpPrefix> = prefixes
            .iter()
            .copied()
            .filter(|prefix| prefix.address.is_ipv4() == ipv4)
            .collect();
        let group = settings.entry(family_name.to_owned()).or_default();
        // NetworkManager ignores address-data when deprecated addresses is present in the same update.
        // GetSettings() may return that legacy key, so remove it before writing address-data.
        group.remove("addresses");
        if family_prefixes.is_empty() {
            group.insert("method".to_owned(), owned_string("disabled"));
            group.remove("address-data");
            group.insert("never-default".to_owned(), OwnedValue::from(true));
            continue;
        }
        group.insert("method".to_owned(), owned_string("manual"));
        group.insert("never-default".to_owned(), OwnedValue::from(true));
        let mut entries = Vec::new();
        for prefix in family_prefixes {
            let mut entry = HashMap::new();
            entry.insert(
                "address".to_owned(),
                owned_string(prefix.address.to_string()),
            );
            entry.insert(
                "prefix".to_owned(),
                OwnedValue::from(u32::from(prefix.prefix_len)),
            );
            entries.push(entry);
        }
        let address_data = Value::from(entries).try_into_owned().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "failed to encode NetworkManager address-data",
                error.to_string(),
            )
        })?;
        group.insert("address-data".to_owned(), address_data);
    }
    Ok(())
}

fn setting_string<'a>(
    settings: &'a HashMap<String, OwnedValue>,
    key: &str,
) -> Result<&'a str, CoreError> {
    settings
        .get(key)
        .and_then(|value| <&str>::try_from(value).ok())
        .ok_or_else(|| {
            CoreError::with_diagnostic(
                ErrorKind::ProtocolError,
                "NetworkManager returned a malformed string setting",
                key,
            )
        })
}

fn optional_setting_string<'a>(
    settings: &'a HashMap<String, OwnedValue>,
    key: &str,
) -> Result<Option<&'a str>, CoreError> {
    let Some(value) = settings.get(key) else {
        return Ok(None);
    };
    <&str>::try_from(value).map(Some).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned a malformed optional string setting",
            error.to_string(),
        )
    })
}

fn owned_string(value: impl Into<String>) -> OwnedValue {
    OwnedValue::from(Str::from(value.into()))
}

fn object_path(value: &str) -> Result<OwnedObjectPath, CoreError> {
    OwnedObjectPath::try_from(value).map_err(|error| {
        network_error(
            ErrorKind::Internal,
            "invalid NetworkManager object path in BlueRoute state",
            error,
        )
    })
}

fn validate_interface_name(interface: &NetworkInterfaceHandle) -> Result<(), CoreError> {
    if interface.as_str().len() > 15 {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidInput,
            "Linux network interface names cannot exceed 15 bytes",
            interface.as_str(),
        ));
    }
    if interface.as_str().contains('/') || interface.as_str().contains('\0') {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidInput,
            "network interface name contains invalid characters",
            interface.as_str(),
        ));
    }
    Ok(())
}

fn short_owner(owner: NetworkId) -> String {
    owner.to_string()[..8].to_owned()
}

fn profile_uuid(owner: NetworkId, interface: &NetworkInterfaceHandle, kind: &str) -> String {
    // Deterministic UUID-shaped identifier derived solely from stable BlueRoute identity and
    // interface purpose. This is an identity key, not a cryptographic primitive.
    let mut bytes = *owner.as_bytes();
    for (index, byte) in interface
        .as_str()
        .bytes()
        .chain([0xff])
        .chain(kind.bytes())
        .enumerate()
    {
        let slot = index % bytes.len();
        let mixed = byte
            .wrapping_add((index as u8).rotate_left((slot % 7) as u32))
            .rotate_left((slot % 5) as u32);
        bytes[slot] = bytes[slot].wrapping_mul(31).wrapping_add(mixed);
        bytes[(slot + 7) % 16] ^= mixed.rotate_left(3);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

async fn connection_snapshot(
    connection: &Connection,
) -> Result<BTreeMap<NetworkConnectionHandle, NetworkConnection>, CoreError> {
    Ok(list_connections(connection)
        .await?
        .into_iter()
        .map(|value| (value.handle.clone(), value))
        .collect())
}

async fn device_snapshot(
    connection: &Connection,
) -> Result<BTreeMap<NetworkDeviceHandle, NetworkDevice>, CoreError> {
    Ok(list_devices(connection)
        .await?
        .into_iter()
        .map(|value| (value.handle.clone(), value))
        .collect())
}

fn queue_connection_changes(
    previous: &BTreeMap<NetworkConnectionHandle, NetworkConnection>,
    current: &BTreeMap<NetworkConnectionHandle, NetworkConnection>,
    pending: &mut VecDeque<NetworkStateEvent>,
) -> Result<(), CoreError> {
    for (handle, old) in previous {
        match current.get(handle) {
            None => push_event(
                pending,
                NetworkStateEvent::ConnectionRemoved(handle.clone()),
            )?,
            Some(new) if old != new => {
                push_event(pending, NetworkStateEvent::ConnectionChanged(new.clone()))?
            }
            Some(_) => {}
        }
    }
    for (handle, value) in current {
        if !previous.contains_key(handle) {
            push_event(pending, NetworkStateEvent::ConnectionAdded(value.clone()))?;
        }
    }
    Ok(())
}

fn queue_device_changes(
    previous: &BTreeMap<NetworkDeviceHandle, NetworkDevice>,
    current: &BTreeMap<NetworkDeviceHandle, NetworkDevice>,
    pending: &mut VecDeque<NetworkStateEvent>,
) -> Result<(), CoreError> {
    for (handle, old) in previous {
        match current.get(handle) {
            None => push_event(pending, NetworkStateEvent::DeviceRemoved(handle.clone()))?,
            Some(new) if old != new => {
                push_event(pending, NetworkStateEvent::DeviceChanged(new.clone()))?
            }
            Some(_) => {}
        }
    }
    for (handle, value) in current {
        if !previous.contains_key(handle) {
            push_event(pending, NetworkStateEvent::DeviceAdded(value.clone()))?;
        }
    }
    Ok(())
}

fn push_event(
    pending: &mut VecDeque<NetworkStateEvent>,
    event: NetworkStateEvent,
) -> Result<(), CoreError> {
    if pending.len() >= EVENT_QUEUE_CAPACITY {
        return Err(CoreError::new(
            ErrorKind::Internal,
            "NetworkManager change queue exceeded its bounded capacity",
        ));
    }
    pending.push_back(event);
    Ok(())
}

fn network_error(
    kind: ErrorKind,
    message: impl Into<String>,
    error: impl std::fmt::Display,
) -> CoreError {
    CoreError::with_diagnostic(kind, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn interface(value: &str) -> NetworkInterfaceHandle {
        NetworkInterfaceHandle::new(value).unwrap()
    }

    #[test]
    fn generated_profile_uuid_is_stable_scoped_and_rfc4122_shaped() {
        let owner = network(7);
        let first = profile_uuid(owner, &interface("br-blue0"), KIND_BRIDGE);
        assert_eq!(
            first,
            profile_uuid(owner, &interface("br-blue0"), KIND_BRIDGE)
        );
        assert_ne!(
            first,
            profile_uuid(owner, &interface("br-blue1"), KIND_BRIDGE)
        );
        assert_ne!(
            first,
            profile_uuid(owner, &interface("br-blue0"), KIND_INTERFACE)
        );
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "5");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn owned_profile_has_explicit_metadata_and_no_gateway_side_effects() {
        let owner = network(3);
        let settings = base_owned_settings(owner, &interface("br-blue0"), KIND_BRIDGE).unwrap();
        assert_eq!(profile_owner(&settings).unwrap(), Some(owner));
        assert_eq!(
            setting_string(settings.get(IPV4_SETTING).unwrap(), "method").unwrap(),
            "disabled"
        );
        assert_eq!(
            setting_string(settings.get(IPV6_SETTING).unwrap(), "method").unwrap(),
            "disabled"
        );
        assert!(!settings.contains_key("proxy"));
    }

    #[test]
    fn foreign_profile_is_not_adopted_as_blueroute_state() {
        let mut settings = HashMap::new();
        settings.insert(CONNECTION_SETTING.to_owned(), HashMap::new());
        assert_eq!(profile_owner(&settings).unwrap(), None);
    }

    #[test]
    fn malformed_blueroute_owner_fails_closed() {
        let mut data = HashMap::<String, String>::new();
        data.insert(OWNER_KEY.to_owned(), "not-an-id".to_owned());
        data.insert(SCHEMA_KEY.to_owned(), OWNERSHIP_SCHEMA.to_owned());
        let mut user = HashMap::new();
        user.insert(USER_DATA_PROPERTY.to_owned(), OwnedValue::from(data));
        let mut settings = HashMap::new();
        settings.insert(USER_SETTING.to_owned(), user);
        assert_eq!(
            profile_owner(&settings).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn incomplete_or_unknown_blueroute_kind_fails_closed() {
        let owner = network(9);
        let mut data = HashMap::<String, String>::new();
        data.insert(OWNER_KEY.to_owned(), owner.to_string());
        data.insert(SCHEMA_KEY.to_owned(), OWNERSHIP_SCHEMA.to_owned());
        let mut user = HashMap::new();
        user.insert(
            USER_DATA_PROPERTY.to_owned(),
            OwnedValue::from(data.clone()),
        );
        let mut settings = HashMap::new();
        settings.insert(USER_SETTING.to_owned(), user);
        assert_eq!(
            profile_owner(&settings).unwrap_err().kind(),
            ErrorKind::InvalidState
        );

        data.insert(KIND_KEY.to_owned(), "unknown".to_owned());
        let mut user = HashMap::new();
        user.insert(USER_DATA_PROPERTY.to_owned(), OwnedValue::from(data));
        settings.insert(USER_SETTING.to_owned(), user);
        assert_eq!(
            profile_owner(&settings).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn address_mutation_preserves_families_and_is_idempotent() {
        let owner = network(4);
        let mut settings = base_owned_settings(owner, &interface("bnep0"), KIND_INTERFACE).unwrap();
        let v4 = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 2)), 24).unwrap();
        let v6 = IpPrefix::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 128).unwrap();
        set_address_prefixes(&mut settings, &[v4, v6]).unwrap();
        let mut read = address_prefixes(&settings).unwrap();
        read.sort();
        assert_eq!(read, vec![v4, v6]);
        set_address_prefixes(&mut settings, &[v4, v6]).unwrap();
        assert_eq!(address_prefixes(&settings).unwrap(), vec![v4, v6]);
        set_address_prefixes(&mut settings, &[v6]).unwrap();
        assert_eq!(address_prefixes(&settings).unwrap(), vec![v6]);
        assert_eq!(
            setting_string(settings.get(IPV4_SETTING).unwrap(), "method").unwrap(),
            "disabled"
        );
    }

    #[test]
    fn address_mutation_removes_legacy_addresses_property() {
        let owner = network(5);
        let mut settings = base_owned_settings(owner, &interface("br-blue0"), KIND_BRIDGE).unwrap();
        settings
            .get_mut(IPV4_SETTING)
            .unwrap()
            .insert("addresses".to_owned(), OwnedValue::from(true));
        let prefix = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 1)), 24).unwrap();
        set_address_prefixes(&mut settings, &[prefix]).unwrap();
        assert!(
            !settings
                .get(IPV4_SETTING)
                .unwrap()
                .contains_key("addresses")
        );
        assert_eq!(address_prefixes(&settings).unwrap(), vec![prefix]);
    }

    #[test]
    fn duplicate_owned_profiles_fail_closed() {
        let owner = network(1);
        let iface = interface("br-blue0");
        let make = |path: &str| OwnedProfile {
            path: path.to_owned(),
            connection: NetworkConnection {
                handle: NetworkConnectionHandle::new(path).unwrap(),
                id: path.to_owned(),
                uuid: profile_uuid(owner, &iface, KIND_BRIDGE),
                connection_type: KIND_BRIDGE.to_owned(),
                interface: Some(iface.clone()),
                owner: Some(owner),
            },
            settings: base_owned_settings(owner, &iface, KIND_BRIDGE).unwrap(),
        };
        let error = exactly_one_owned_profile(vec![make("/one"), make("/two")], owner, &iface)
            .err()
            .expect("duplicates must fail closed");
        assert_eq!(error.kind(), ErrorKind::InvalidState);
    }

    #[test]
    fn event_diff_reports_removal_change_then_addition_deterministically() {
        let handle_a = NetworkDeviceHandle::new("/a").unwrap();
        let handle_b = NetworkDeviceHandle::new("/b").unwrap();
        let handle_c = NetworkDeviceHandle::new("/c").unwrap();
        let device = |handle: NetworkDeviceHandle, state: u32| NetworkDevice {
            handle,
            interface: interface("eth0"),
            managed: true,
            device_type: 1,
            state,
            active_connection: None,
        };
        let previous = BTreeMap::from([
            (handle_a.clone(), device(handle_a.clone(), 10)),
            (handle_b.clone(), device(handle_b.clone(), 20)),
        ]);
        let current = BTreeMap::from([
            (handle_b.clone(), device(handle_b.clone(), 30)),
            (handle_c.clone(), device(handle_c.clone(), 40)),
        ]);
        let mut pending = VecDeque::new();
        queue_device_changes(&previous, &current, &mut pending).unwrap();
        assert!(matches!(
            pending.pop_front(),
            Some(NetworkStateEvent::DeviceRemoved(handle)) if handle == handle_a
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(NetworkStateEvent::DeviceChanged(device)) if device.handle == handle_b
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(NetworkStateEvent::DeviceAdded(device)) if device.handle == handle_c
        ));
    }

    #[test]
    fn overlong_interface_name_is_rejected_before_dbus() {
        let error = validate_interface_name(&interface("abcdefghijklmnop")).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

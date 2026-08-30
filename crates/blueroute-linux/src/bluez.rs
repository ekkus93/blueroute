use std::collections::{BTreeMap, HashMap, VecDeque};

use futures_lite::StreamExt;
use zbus::fdo::{DBusProxy, ManagedObjects, ObjectManagerProxy};
use zbus::message::Type as MessageType;
use zbus::names::{BusName, OwnedInterfaceName};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, Message, MessageStream, Proxy};

use blueroute_core::{CoreError, ErrorKind};

use crate::{
    AdapterEventSubscription, AdapterHandle, BackendFuture, BluetoothAdapter,
    BluetoothAdapterEvent, BluetoothBackend, BluetoothPeerEvent, DiscoveredPeer,
    PeerEventSubscription, PeerHandle,
};

const BLUEZ_SERVICE: &str = "org.bluez";
const BLUEZ_ROOT_PATH: &str = "/";
const BLUEZ_OBJECT_PREFIX: &str = "/org/bluez/";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const DEVICE_INTERFACE: &str = "org.bluez.Device1";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const INTERFACES_ADDED: &str = "InterfacesAdded";
const INTERFACES_REMOVED: &str = "InterfacesRemoved";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const POWERED_PROPERTY: &str = "Powered";
const ALIAS_PROPERTY: &str = "Alias";
const NAME_PROPERTY: &str = "Name";
const PAIRED_PROPERTY: &str = "Paired";
const TRUSTED_PROPERTY: &str = "Trusted";
const START_DISCOVERY_METHOD: &str = "StartDiscovery";
const STOP_DISCOVERY_METHOD: &str = "StopDiscovery";

/// Production BlueZ backend using the Linux system D-Bus directly.
#[derive(Clone, Debug)]
pub struct BluezBackend {
    connection: Connection,
}

impl BluezBackend {
    /// Connects to the system bus and verifies that `org.bluez` currently has an owner.
    pub async fn connect_system() -> Result<Self, CoreError> {
        let connection = Connection::system()
            .await
            .map_err(|error| bluez_error("failed to connect to the system D-Bus", error))?;
        ensure_bluez_available(&connection).await?;
        Ok(Self { connection })
    }

    /// Returns whether the BlueZ service currently owns its well-known bus name.
    pub async fn service_available(&self) -> Result<bool, CoreError> {
        bluez_service_available(&self.connection).await
    }

    async fn snapshot(&self) -> Result<Vec<BluetoothAdapter>, CoreError> {
        enumerate_adapters(&self.connection).await
    }
}

impl BluetoothBackend for BluezBackend {
    fn adapters(&self) -> BackendFuture<'_, Vec<BluetoothAdapter>> {
        Box::pin(async move { self.snapshot().await })
    }

    fn subscribe_adapter_events(&self) -> BackendFuture<'_, Box<dyn AdapterEventSubscription>> {
        Box::pin(async move {
            ensure_bluez_available(&self.connection).await?;
            let stream = bluez_signal_stream(&self.connection).await?;
            let snapshot = self.snapshot().await?;
            Ok(Box::new(BluezAdapterSubscription {
                connection: self.connection.clone(),
                stream,
                snapshot,
                pending: VecDeque::new(),
            }) as Box<dyn AdapterEventSubscription>)
        })
    }

    fn start_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            call_discovery_method(&self.connection, &adapter, START_DISCOVERY_METHOD).await
        })
    }

    fn stop_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            call_discovery_method(&self.connection, &adapter, STOP_DISCOVERY_METHOD).await
        })
    }

    fn discovered_peers(&self, adapter: AdapterHandle) -> BackendFuture<'_, Vec<DiscoveredPeer>> {
        Box::pin(async move { enumerate_peers(&self.connection, &adapter).await })
    }

    fn subscribe_peer_events(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, Box<dyn PeerEventSubscription>> {
        Box::pin(async move {
            ensure_bluez_available(&self.connection).await?;
            let stream = bluez_signal_stream(&self.connection).await?;
            ensure_adapter_exists(&self.connection, &adapter).await?;
            Ok(Box::new(BluezPeerSubscription {
                connection: self.connection.clone(),
                stream,
                adapter,
            }) as Box<dyn PeerEventSubscription>)
        })
    }

    fn pair(&self, _peer: PeerHandle) -> BackendFuture<'_, ()> {
        unsupported_future("Bluetooth pairing is not implemented until P4-004")
    }

    fn set_trusted(&self, _peer: PeerHandle, _trusted: bool) -> BackendFuture<'_, ()> {
        unsupported_future("Bluetooth trust management is not implemented until P4-004")
    }
}

struct BluezAdapterSubscription {
    connection: Connection,
    stream: MessageStream,
    snapshot: Vec<BluetoothAdapter>,
    pending: VecDeque<BluetoothAdapterEvent>,
}

impl AdapterEventSubscription for BluezAdapterSubscription {
    fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothAdapterEvent>> {
        Box::pin(async move {
            loop {
                if let Some(event) = self.pending.pop_front() {
                    return Ok(Some(event));
                }

                let Some(message) = self.stream.next().await else {
                    return Ok(None);
                };
                let message = message.map_err(|error| {
                    bluez_error("failed while receiving BlueZ adapter changes", error)
                })?;
                if !is_adapter_change_signal(&message)? {
                    continue;
                }

                let next = enumerate_adapters(&self.connection).await?;
                self.pending = diff_adapter_snapshots(&self.snapshot, &next);
                self.snapshot = next;
            }
        })
    }
}

struct BluezPeerSubscription {
    connection: Connection,
    stream: MessageStream,
    adapter: AdapterHandle,
}

impl PeerEventSubscription for BluezPeerSubscription {
    fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothPeerEvent>> {
        Box::pin(async move {
            loop {
                let Some(message) = self.stream.next().await else {
                    return Ok(None);
                };
                let message = message.map_err(|error| {
                    bluez_error("failed while receiving BlueZ peer changes", error)
                })?;
                if let Some(event) =
                    peer_event_from_signal(&self.connection, &self.adapter, &message).await?
                {
                    return Ok(Some(event));
                }
            }
        })
    }
}

async fn bluez_service_available(connection: &Connection) -> Result<bool, CoreError> {
    let proxy = DBusProxy::new(connection)
        .await
        .map_err(|error| bluez_error("failed to create a system D-Bus service proxy", error))?;
    let service_name = BusName::try_from(BLUEZ_SERVICE).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::Internal,
            "BlueZ service name is invalid",
            error.to_string(),
        )
    })?;
    proxy
        .name_has_owner(service_name)
        .await
        .map_err(|error| bluez_error("failed to query BlueZ service availability", error))
}

async fn ensure_bluez_available(connection: &Connection) -> Result<(), CoreError> {
    if bluez_service_available(connection).await? {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::BluezUnavailable,
            "BlueZ is not available on the system D-Bus",
        ))
    }
}

async fn managed_objects(connection: &Connection) -> Result<ManagedObjects, CoreError> {
    ensure_bluez_available(connection).await?;
    let proxy = ObjectManagerProxy::new(connection, BLUEZ_SERVICE, BLUEZ_ROOT_PATH)
        .await
        .map_err(|error| bluez_error("failed to create the BlueZ object-manager proxy", error))?;
    proxy
        .get_managed_objects()
        .await
        .map_err(|error| bluez_error("failed to enumerate BlueZ managed objects", error))
}

async fn enumerate_adapters(connection: &Connection) -> Result<Vec<BluetoothAdapter>, CoreError> {
    let objects = managed_objects(connection).await?;
    adapters_from_managed_objects(&objects)
}

async fn ensure_adapter_exists(
    connection: &Connection,
    adapter: &AdapterHandle,
) -> Result<BluetoothAdapter, CoreError> {
    enumerate_adapters(connection)
        .await?
        .into_iter()
        .find(|candidate| candidate.handle == *adapter)
        .ok_or_else(|| {
            CoreError::new(
                ErrorKind::MissingAdapter,
                "Bluetooth adapter is no longer available",
            )
        })
}

async fn call_discovery_method(
    connection: &Connection,
    adapter: &AdapterHandle,
    method: &'static str,
) -> Result<(), CoreError> {
    let current = ensure_adapter_exists(connection, adapter).await?;
    if method == START_DISCOVERY_METHOD && !current.powered {
        return Err(CoreError::new(
            ErrorKind::AdapterDisabled,
            "Bluetooth adapter must be powered before discovery can start",
        ));
    }

    let proxy = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        current.handle.as_str(),
        ADAPTER_INTERFACE,
    )
    .await
    .map_err(|error| discovery_error(method, error))?;
    proxy
        .call_method(method, &())
        .await
        .map_err(|error| discovery_error(method, error))?;
    Ok(())
}

fn discovery_error(method: &'static str, error: zbus::Error) -> CoreError {
    let kind = match &error {
        zbus::Error::MethodError(name, _, _) => discovery_method_error_kind(name.as_str()),
        _ => ErrorKind::BluezUnavailable,
    };
    let operation = if method == START_DISCOVERY_METHOD {
        "start Bluetooth discovery"
    } else {
        "stop Bluetooth discovery"
    };
    CoreError::with_diagnostic(kind, format!("failed to {operation}"), error.to_string())
}

fn discovery_method_error_kind(name: &str) -> ErrorKind {
    match name {
        "org.bluez.Error.NotReady" => ErrorKind::AdapterDisabled,
        "org.bluez.Error.InProgress" => ErrorKind::InvalidState,
        "org.bluez.Error.NotAuthorized" => ErrorKind::CapabilityUnavailable,
        "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject" => {
            ErrorKind::MissingAdapter
        }
        _ => ErrorKind::CapabilityUnavailable,
    }
}

async fn enumerate_peers(
    connection: &Connection,
    adapter: &AdapterHandle,
) -> Result<Vec<DiscoveredPeer>, CoreError> {
    let objects = managed_objects(connection).await?;
    let adapter_exists = adapters_from_managed_objects(&objects)?
        .into_iter()
        .any(|candidate| candidate.handle == *adapter);
    if !adapter_exists {
        return Err(CoreError::new(
            ErrorKind::MissingAdapter,
            "Bluetooth adapter is no longer available",
        ));
    }
    peers_from_managed_objects(&objects, adapter)
}

fn adapters_from_managed_objects(
    objects: &ManagedObjects,
) -> Result<Vec<BluetoothAdapter>, CoreError> {
    let mut adapters = Vec::new();
    for (path, interfaces) in objects {
        let Some(properties) = interfaces.iter().find_map(|(name, properties)| {
            (name.as_str() == ADAPTER_INTERFACE).then_some(properties)
        }) else {
            continue;
        };

        let powered = properties
            .get(POWERED_PROPERTY)
            .ok_or_else(|| {
                CoreError::new(
                    ErrorKind::ProtocolError,
                    "BlueZ adapter is missing its Powered property",
                )
            })
            .and_then(owned_value_to_bool)?;
        let handle = AdapterHandle::new(path.as_str())?;
        adapters.push(BluetoothAdapter { handle, powered });
    }
    adapters.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(adapters)
}

fn peers_from_managed_objects(
    objects: &ManagedObjects,
    adapter: &AdapterHandle,
) -> Result<Vec<DiscoveredPeer>, CoreError> {
    let mut peers = Vec::new();
    for (path, interfaces) in objects {
        if !is_device_object_path_for_adapter(path.as_str(), adapter) {
            continue;
        }
        let Some(properties) = interfaces.iter().find_map(|(name, properties)| {
            (name.as_str() == DEVICE_INTERFACE).then_some(properties)
        }) else {
            continue;
        };
        peers.push(peer_from_properties(path, properties)?);
    }
    peers.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(peers)
}

fn peer_from_properties(
    path: &OwnedObjectPath,
    properties: &HashMap<String, OwnedValue>,
) -> Result<DiscoveredPeer, CoreError> {
    let paired = required_peer_bool(properties, PAIRED_PROPERTY)?;
    let trusted = required_peer_bool(properties, TRUSTED_PROPERTY)?;
    let display_name = optional_peer_string(properties, ALIAS_PROPERTY)?
        .or(optional_peer_string(properties, NAME_PROPERTY)?);
    Ok(DiscoveredPeer {
        handle: PeerHandle::new(path.as_str())?,
        display_name,
        paired,
        trusted,
    })
}

fn required_peer_bool(
    properties: &HashMap<String, OwnedValue>,
    property: &'static str,
) -> Result<bool, CoreError> {
    let value = properties.get(property).ok_or_else(|| {
        CoreError::new(
            ErrorKind::ProtocolError,
            format!("BlueZ device is missing its {property} property"),
        )
    })?;
    bool::try_from(value).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            format!("BlueZ device {property} property is not a boolean"),
            error.to_string(),
        )
    })
}

fn optional_peer_string(
    properties: &HashMap<String, OwnedValue>,
    property: &'static str,
) -> Result<Option<String>, CoreError> {
    let Some(value) = properties.get(property) else {
        return Ok(None);
    };
    let value = <&str>::try_from(value).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            format!("BlueZ device {property} property is not a string"),
            error.to_string(),
        )
    })?;
    if value.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(value.to_owned()))
    }
}

fn is_device_object_path_for_adapter(path: &str, adapter: &AdapterHandle) -> bool {
    let prefix = format!("{}/", adapter.as_str().trim_end_matches('/'));
    let Some(suffix) = path.strip_prefix(&prefix) else {
        return false;
    };
    suffix.starts_with("dev_") && !suffix.contains('/')
}

fn owned_value_to_bool(value: &OwnedValue) -> Result<bool, CoreError> {
    bool::try_from(value).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "BlueZ adapter Powered property is not a boolean",
            error.to_string(),
        )
    })
}

async fn bluez_signal_stream(connection: &Connection) -> Result<MessageStream, CoreError> {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BLUEZ_SERVICE)
        .map_err(|error| bluez_error("failed to build the BlueZ signal subscription", error))?
        .build();
    MessageStream::for_match_rule(rule, connection, Some(64))
        .await
        .map_err(|error| bluez_error("failed to subscribe to BlueZ changes", error))
}

fn is_adapter_change_signal(message: &Message) -> Result<bool, CoreError> {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str());
    let member = header.member().map(|name| name.as_str());

    match (interface, member) {
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_ADDED)) => {
            let (path, interfaces): (
                OwnedObjectPath,
                HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ InterfacesAdded signal", error)
            })?;
            Ok(is_adapter_object_path(path.as_str())
                && interfaces
                    .keys()
                    .any(|name| name.as_str() == ADAPTER_INTERFACE))
        }
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_REMOVED)) => {
            let (path, interfaces): (OwnedObjectPath, Vec<OwnedInterfaceName>) =
                message.body().deserialize().map_err(|error| {
                    bluez_error("failed to decode BlueZ InterfacesRemoved signal", error)
                })?;
            Ok(is_adapter_object_path(path.as_str())
                && interfaces
                    .iter()
                    .any(|name| name.as_str() == ADAPTER_INTERFACE))
        }
        (Some(PROPERTIES_INTERFACE), Some(PROPERTIES_CHANGED)) => {
            let Some(path) = header.path() else {
                return Ok(false);
            };
            if !is_adapter_object_path(path.as_str()) {
                return Ok(false);
            }
            let (interface_name, changed, invalidated): (
                OwnedInterfaceName,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ PropertiesChanged signal", error)
            })?;
            Ok(interface_name.as_str() == ADAPTER_INTERFACE
                && (changed.contains_key(POWERED_PROPERTY)
                    || invalidated.iter().any(|name| name == POWERED_PROPERTY)))
        }
        _ => Ok(false),
    }
}

async fn peer_event_from_signal(
    connection: &Connection,
    adapter: &AdapterHandle,
    message: &Message,
) -> Result<Option<BluetoothPeerEvent>, CoreError> {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str());
    let member = header.member().map(|name| name.as_str());

    match (interface, member) {
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_ADDED)) => {
            let (path, interfaces): (
                OwnedObjectPath,
                HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ InterfacesAdded signal", error)
            })?;
            if !is_device_object_path_for_adapter(path.as_str(), adapter) {
                return Ok(None);
            }
            let Some(properties) = interfaces.iter().find_map(|(name, properties)| {
                (name.as_str() == DEVICE_INTERFACE).then_some(properties)
            }) else {
                return Ok(None);
            };
            Ok(Some(BluetoothPeerEvent::Added(peer_from_properties(
                &path, properties,
            )?)))
        }
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_REMOVED)) => {
            let (path, interfaces): (OwnedObjectPath, Vec<OwnedInterfaceName>) =
                message.body().deserialize().map_err(|error| {
                    bluez_error("failed to decode BlueZ InterfacesRemoved signal", error)
                })?;
            if !is_device_object_path_for_adapter(path.as_str(), adapter)
                || !interfaces
                    .iter()
                    .any(|name| name.as_str() == DEVICE_INTERFACE)
            {
                return Ok(None);
            }
            Ok(Some(BluetoothPeerEvent::Removed(PeerHandle::new(
                path.as_str(),
            )?)))
        }
        (Some(PROPERTIES_INTERFACE), Some(PROPERTIES_CHANGED)) => {
            let Some(path) = header.path() else {
                return Ok(None);
            };
            if !is_device_object_path_for_adapter(path.as_str(), adapter) {
                return Ok(None);
            }
            let (interface_name, changed, invalidated): (
                OwnedInterfaceName,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ PropertiesChanged signal", error)
            })?;
            if interface_name.as_str() != DEVICE_INTERFACE
                || !peer_properties_changed(&changed, &invalidated)
            {
                return Ok(None);
            }
            let Some(peer) = fetch_peer(connection, adapter, path.as_str()).await? else {
                return Ok(None);
            };
            Ok(Some(BluetoothPeerEvent::Changed(peer)))
        }
        _ => Ok(None),
    }
}

fn peer_properties_changed(changed: &HashMap<String, OwnedValue>, invalidated: &[String]) -> bool {
    [
        ALIAS_PROPERTY,
        NAME_PROPERTY,
        PAIRED_PROPERTY,
        TRUSTED_PROPERTY,
    ]
    .iter()
    .any(|property| {
        changed.contains_key(*property)
            || invalidated
                .iter()
                .any(|invalid| invalid.as_str() == *property)
    })
}

async fn fetch_peer(
    connection: &Connection,
    adapter: &AdapterHandle,
    path: &str,
) -> Result<Option<DiscoveredPeer>, CoreError> {
    let objects = managed_objects(connection).await?;
    let Some((object_path, interfaces)) = objects
        .iter()
        .find(|(candidate, _)| candidate.as_str() == path)
    else {
        return Ok(None);
    };
    if !is_device_object_path_for_adapter(object_path.as_str(), adapter) {
        return Ok(None);
    }
    let Some(properties) = interfaces
        .iter()
        .find_map(|(name, properties)| (name.as_str() == DEVICE_INTERFACE).then_some(properties))
    else {
        return Ok(None);
    };
    peer_from_properties(object_path, properties).map(Some)
}

fn is_adapter_object_path(path: &str) -> bool {
    let Some(suffix) = path.strip_prefix(BLUEZ_OBJECT_PREFIX) else {
        return false;
    };
    !suffix.is_empty() && !suffix.contains('/')
}

fn diff_adapter_snapshots(
    previous: &[BluetoothAdapter],
    current: &[BluetoothAdapter],
) -> VecDeque<BluetoothAdapterEvent> {
    let previous = previous
        .iter()
        .map(|adapter| (adapter.handle.clone(), adapter))
        .collect::<BTreeMap<_, _>>();
    let current = current
        .iter()
        .map(|adapter| (adapter.handle.clone(), adapter))
        .collect::<BTreeMap<_, _>>();
    let mut events = VecDeque::new();

    for (handle, old) in &previous {
        if !current.contains_key(handle) {
            events.push_back(BluetoothAdapterEvent::Removed(handle.clone()));
        } else if let Some(new) = current.get(handle)
            && old.powered != new.powered
        {
            events.push_back(BluetoothAdapterEvent::PoweredChanged {
                handle: handle.clone(),
                powered: new.powered,
            });
        }
    }
    for (handle, adapter) in &current {
        if !previous.contains_key(handle) {
            events.push_back(BluetoothAdapterEvent::Added((*adapter).clone()));
        }
    }
    events
}

fn unsupported_future(message: &'static str) -> BackendFuture<'static, ()> {
    Box::pin(async move { Err(CoreError::new(ErrorKind::CapabilityUnavailable, message)) })
}

fn bluez_error(message: &'static str, error: impl std::fmt::Display) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::BluezUnavailable, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Str;

    fn adapter(path: &str, powered: bool) -> BluetoothAdapter {
        BluetoothAdapter {
            handle: AdapterHandle::new(path).unwrap(),
            powered,
        }
    }

    fn peer_properties(
        alias: Option<&str>,
        name: Option<&str>,
        paired: bool,
        trusted: bool,
    ) -> HashMap<String, OwnedValue> {
        let mut properties = HashMap::new();
        if let Some(alias) = alias {
            properties.insert(
                ALIAS_PROPERTY.to_owned(),
                OwnedValue::from(Str::from(alias)),
            );
        }
        if let Some(name) = name {
            properties.insert(NAME_PROPERTY.to_owned(), OwnedValue::from(Str::from(name)));
        }
        properties.insert(PAIRED_PROPERTY.to_owned(), OwnedValue::from(paired));
        properties.insert(TRUSTED_PROPERTY.to_owned(), OwnedValue::from(trusted));
        properties
    }

    fn add_device(
        objects: &mut ManagedObjects,
        path: &str,
        properties: HashMap<String, OwnedValue>,
    ) {
        let mut interfaces = HashMap::new();
        interfaces.insert(
            OwnedInterfaceName::try_from(DEVICE_INTERFACE).unwrap(),
            properties,
        );
        objects.insert(OwnedObjectPath::try_from(path).unwrap(), interfaces);
    }

    #[test]
    fn device_paths_are_scoped_to_one_adapter_and_one_object_level() {
        let hci0 = AdapterHandle::new("/org/bluez/hci0").unwrap();
        assert!(is_device_object_path_for_adapter(
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF",
            &hci0
        ));
        assert!(!is_device_object_path_for_adapter(
            "/org/bluez/hci1/dev_AA_BB_CC_DD_EE_FF",
            &hci0
        ));
        assert!(!is_device_object_path_for_adapter(
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/service0001",
            &hci0
        ));
        assert!(!is_device_object_path_for_adapter(
            "/org/bluez/hci0/not_a_device",
            &hci0
        ));
    }

    #[test]
    fn peer_mapping_prefers_alias_and_falls_back_to_name() {
        let path = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let aliased = peer_from_properties(
            &path,
            &peer_properties(Some("Friendly"), Some("Remote Name"), true, false),
        )
        .unwrap();
        assert_eq!(aliased.display_name.as_deref(), Some("Friendly"));
        assert!(aliased.paired);
        assert!(!aliased.trusted);

        let named = peer_from_properties(
            &path,
            &peer_properties(None, Some("Remote Name"), false, true),
        )
        .unwrap();
        assert_eq!(named.display_name.as_deref(), Some("Remote Name"));
        assert!(!named.paired);
        assert!(named.trusted);
    }

    #[test]
    fn peer_mapping_rejects_missing_required_boolean_properties() {
        let path = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let mut properties = peer_properties(Some("Peer"), None, false, false);
        properties.remove(PAIRED_PROPERTY);
        let error = peer_from_properties(&path, &properties).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ProtocolError);
        assert!(error.message().contains(PAIRED_PROPERTY));
    }

    #[test]
    fn peer_snapshot_filters_adapter_and_sorts_by_stable_handle() {
        let mut objects = ManagedObjects::new();
        add_device(
            &mut objects,
            "/org/bluez/hci0/dev_BB_BB_BB_BB_BB_BB",
            peer_properties(Some("Second"), None, false, false),
        );
        add_device(
            &mut objects,
            "/org/bluez/hci1/dev_00_00_00_00_00_00",
            peer_properties(Some("Other Adapter"), None, false, false),
        );
        add_device(
            &mut objects,
            "/org/bluez/hci0/dev_AA_AA_AA_AA_AA_AA",
            peer_properties(Some("First"), None, true, true),
        );

        let peers =
            peers_from_managed_objects(&objects, &AdapterHandle::new("/org/bluez/hci0").unwrap())
                .unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(
            peers[0].handle.as_str(),
            "/org/bluez/hci0/dev_AA_AA_AA_AA_AA_AA"
        );
        assert_eq!(
            peers[1].handle.as_str(),
            "/org/bluez/hci0/dev_BB_BB_BB_BB_BB_BB"
        );
    }

    #[test]
    fn discovery_method_errors_map_to_typed_core_errors() {
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.NotReady"),
            ErrorKind::AdapterDisabled
        );
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.InProgress"),
            ErrorKind::InvalidState
        );
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.DoesNotExist"),
            ErrorKind::MissingAdapter
        );
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.NotAuthorized"),
            ErrorKind::CapabilityUnavailable
        );
    }

    #[test]
    fn peer_change_filter_ignores_unmodeled_high_rate_properties() {
        let mut changed = HashMap::new();
        changed.insert("RSSI".to_owned(), OwnedValue::from(-42_i16));
        assert!(!peer_properties_changed(&changed, &[]));

        changed.insert(PAIRED_PROPERTY.to_owned(), OwnedValue::from(true));
        assert!(peer_properties_changed(&changed, &[]));
        assert!(peer_properties_changed(
            &HashMap::new(),
            &[ALIAS_PROPERTY.to_owned()]
        ));
    }

    #[test]
    fn adapter_object_paths_exclude_nested_device_objects() {
        assert!(is_adapter_object_path("/org/bluez/hci0"));
        assert!(is_adapter_object_path("/org/bluez/controller-name"));
        assert!(!is_adapter_object_path(
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
        ));
        assert!(!is_adapter_object_path("/org/bluez/"));
        assert!(!is_adapter_object_path("/other/hci0"));
    }

    #[test]
    fn adapter_snapshot_diff_reports_remove_power_and_add_deterministically() {
        let previous = vec![
            adapter("/org/bluez/hci0", false),
            adapter("/org/bluez/hci1", true),
        ];
        let current = vec![
            adapter("/org/bluez/hci0", true),
            adapter("/org/bluez/hci2", false),
        ];

        let events = diff_adapter_snapshots(&previous, &current)
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            vec![
                BluetoothAdapterEvent::PoweredChanged {
                    handle: AdapterHandle::new("/org/bluez/hci0").unwrap(),
                    powered: true,
                },
                BluetoothAdapterEvent::Removed(AdapterHandle::new("/org/bluez/hci1").unwrap()),
                BluetoothAdapterEvent::Added(adapter("/org/bluez/hci2", false)),
            ]
        );
    }

    #[test]
    fn unchanged_snapshot_produces_no_events() {
        let snapshot = vec![adapter("/org/bluez/hci0", true)];
        assert!(diff_adapter_snapshots(&snapshot, &snapshot).is_empty());
    }
}

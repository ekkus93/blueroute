use std::collections::{BTreeMap, HashMap, VecDeque};

use futures_lite::StreamExt;
use zbus::fdo::{DBusProxy, ManagedObjects, ObjectManagerProxy};
use zbus::message::Type as MessageType;
use zbus::names::{BusName, OwnedInterfaceName};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, Message, MessageStream};

use blueroute_core::{CoreError, ErrorKind};

use crate::{
    AdapterEventSubscription, AdapterHandle, BackendFuture, BluetoothAdapter,
    BluetoothAdapterEvent, BluetoothBackend, DiscoveredPeer, PeerHandle,
};

const BLUEZ_SERVICE: &str = "org.bluez";
const BLUEZ_ROOT_PATH: &str = "/";
const BLUEZ_OBJECT_PREFIX: &str = "/org/bluez/";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const INTERFACES_ADDED: &str = "InterfacesAdded";
const INTERFACES_REMOVED: &str = "InterfacesRemoved";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const POWERED_PROPERTY: &str = "Powered";

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

    fn start_discovery(&self, _adapter: AdapterHandle) -> BackendFuture<'_, ()> {
        unsupported_future("Bluetooth device discovery is not implemented until P4-003")
    }

    fn stop_discovery(&self, _adapter: AdapterHandle) -> BackendFuture<'_, ()> {
        unsupported_future("Bluetooth device discovery is not implemented until P4-003")
    }

    fn discovered_peers(&self, _adapter: AdapterHandle) -> BackendFuture<'_, Vec<DiscoveredPeer>> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "Bluetooth device discovery is not implemented until P4-003",
            ))
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

async fn enumerate_adapters(connection: &Connection) -> Result<Vec<BluetoothAdapter>, CoreError> {
    ensure_bluez_available(connection).await?;
    let proxy = ObjectManagerProxy::new(connection, BLUEZ_SERVICE, BLUEZ_ROOT_PATH)
        .await
        .map_err(|error| bluez_error("failed to create the BlueZ object-manager proxy", error))?;
    let objects = proxy
        .get_managed_objects()
        .await
        .map_err(|error| bluez_error("failed to enumerate BlueZ managed objects", error))?;
    adapters_from_managed_objects(&objects)
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

    fn adapter(path: &str, powered: bool) -> BluetoothAdapter {
        BluetoothAdapter {
            handle: AdapterHandle::new(path).unwrap(),
            powered,
        }
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

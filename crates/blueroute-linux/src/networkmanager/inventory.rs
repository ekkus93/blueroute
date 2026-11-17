//! Read-only inventory of NetworkManager connections and devices, and the change events derived from successive snapshots.

use super::dbus::{
    active_connection_proxy, device_proxy, ensure_networkmanager_available, network_error,
    network_manager_proxy, optional_setting_string, setting_string, settings_connection_proxy,
    settings_proxy,
};
use super::ownership::profile_owner;
use super::{CONNECTION_SETTING, EVENT_QUEUE_CAPACITY, NmSettings, RECONCILE_INTERVAL};
use crate::{
    BackendFuture, NetworkConnection, NetworkConnectionHandle, NetworkDevice, NetworkDeviceHandle,
    NetworkInterfaceHandle, NetworkStateEvent, NetworkStateSubscription,
};
use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind};
use std::collections::{BTreeMap, VecDeque};
use zbus::Connection;
use zbus::zvariant::OwnedObjectPath;

pub(super) struct NetworkManagerSubscription {
    pub(super) connection: Connection,
    pub(super) connections: BTreeMap<NetworkConnectionHandle, NetworkConnection>,
    pub(super) devices: BTreeMap<NetworkDeviceHandle, NetworkDevice>,
    pub(super) pending: VecDeque<NetworkStateEvent>,
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

pub(super) async fn list_connections(
    connection: &Connection,
) -> Result<Vec<NetworkConnection>, CoreError> {
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

pub(super) async fn list_devices(connection: &Connection) -> Result<Vec<NetworkDevice>, CoreError> {
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

pub(super) async fn get_settings(
    connection: &Connection,
    path: &str,
) -> Result<NmSettings, CoreError> {
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

pub(super) fn connection_from_settings(
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

pub(super) async fn connection_snapshot(
    connection: &Connection,
) -> Result<BTreeMap<NetworkConnectionHandle, NetworkConnection>, CoreError> {
    Ok(list_connections(connection)
        .await?
        .into_iter()
        .map(|value| (value.handle.clone(), value))
        .collect())
}

pub(super) async fn device_snapshot(
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

pub(super) fn push_event(
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

#[cfg(test)]
mod tests {

    use super::super::test_support::interface;
    use super::*;

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
}

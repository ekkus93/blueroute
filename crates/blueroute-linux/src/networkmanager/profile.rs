//! Lifecycle of BlueRoute-owned profiles: bridge ensure and removal, add, update, activate, reapply, deactivate, and waits for the resulting state.

use super::dbus::{
    device_proxy, network_error, network_manager_proxy, object_path, settings_connection_proxy,
    settings_proxy,
};
use super::inventory::{connection_from_settings, get_settings, list_connections, list_devices};
use super::ownership::{
    base_owned_settings, exactly_one_owned_profile, matching_profiles, profile_owner,
    reject_foreign_interface_claim, validate_interface_name,
};
use super::{
    ADD_CONNECTION2_BLOCK_AUTOCONNECT, ADD_CONNECTION2_TO_DISK, APPLY_TIMEOUT, KIND_BRIDGE,
    NmSettings, RECONCILE_INTERVAL, UPDATE2_TO_DISK,
};
use crate::{NetworkConnection, NetworkConnectionHandle, NetworkDevice, NetworkInterfaceHandle};
use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind, NetworkId};
use std::collections::HashMap;
use std::time::Instant;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

pub(super) async fn ensure_bridge(
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

pub(super) async fn remove_owned_interface(
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

pub(super) async fn add_owned_profile(
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

pub(super) async fn update_profile(
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

pub(super) async fn activate_profile(
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

pub(super) async fn reapply_if_active(
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

pub(super) async fn device_by_interface(
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

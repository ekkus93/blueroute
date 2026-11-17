//! NetworkManager D-Bus access: availability, typed proxies, settings-map accessors and error mapping.

use super::{
    ACTIVE_CONNECTION_INTERFACE, DEVICE_INTERFACE, NM_INTERFACE, NM_PATH, NM_SERVICE,
    SETTINGS_CONNECTION_INTERFACE, SETTINGS_INTERFACE, SETTINGS_PATH,
};
use blueroute_core::{CoreError, ErrorKind};
use std::collections::HashMap;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Str};
use zbus::{Connection, Proxy};

pub(super) async fn ensure_networkmanager_available(
    connection: &Connection,
) -> Result<(), CoreError> {
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

pub(super) async fn network_manager_proxy(connection: &Connection) -> Result<Proxy<'_>, CoreError> {
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

pub(super) async fn settings_proxy(connection: &Connection) -> Result<Proxy<'_>, CoreError> {
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

pub(super) async fn settings_connection_proxy<'a>(
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

pub(super) async fn device_proxy<'a>(
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

pub(super) async fn active_connection_proxy<'a>(
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

pub(super) fn setting_string<'a>(
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

pub(super) fn optional_setting_string<'a>(
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

pub(super) fn owned_string(value: impl Into<String>) -> OwnedValue {
    OwnedValue::from(Str::from(value.into()))
}

pub(super) fn object_path(value: &str) -> Result<OwnedObjectPath, CoreError> {
    OwnedObjectPath::try_from(value).map_err(|error| {
        network_error(
            ErrorKind::Internal,
            "invalid NetworkManager object path in BlueRoute state",
            error,
        )
    })
}

pub(super) fn network_error(
    kind: ErrorKind,
    message: impl Into<String>,
    error: impl std::fmt::Display,
) -> CoreError {
    CoreError::with_diagnostic(kind, message, error.to_string())
}

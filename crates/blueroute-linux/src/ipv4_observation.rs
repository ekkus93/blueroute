use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;

use blueroute_core::{CoreError, ErrorKind, IpPrefix, normalized_ipv4_prefix};
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy};

use crate::{BackendFuture, IpNetworkObservationBackend, NetworkManagerBackend};

const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_INTERFACE: &str = "org.freedesktop.NetworkManager";
const DEVICE_INTERFACE: &str = "org.freedesktop.NetworkManager.Device";
const IP4_CONFIG_INTERFACE: &str = "org.freedesktop.NetworkManager.IP4Config";

impl IpNetworkObservationBackend for NetworkManagerBackend {
    fn active_ipv4_prefixes(&self) -> BackendFuture<'_, Vec<IpPrefix>> {
        Box::pin(async { active_ipv4_prefixes().await })
    }
}

async fn active_ipv4_prefixes() -> Result<Vec<IpPrefix>, CoreError> {
    let connection = Connection::system().await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to connect to the system D-Bus for IPv4 conflict observation",
            error,
        )
    })?;
    ensure_networkmanager_available(&connection).await?;
    let manager = Proxy::new(&connection, NM_SERVICE, NM_PATH, NM_INTERFACE)
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to create the NetworkManager proxy for IPv4 conflict observation",
                error,
            )
        })?;
    let paths: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to enumerate NetworkManager devices for IPv4 conflict detection",
            error,
        )
    })?;

    let mut prefixes = Vec::new();
    for path in paths {
        let device = Proxy::new(&connection, NM_SERVICE, path.as_str(), DEVICE_INTERFACE)
            .await
            .map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to access a NetworkManager device for IPv4 conflict detection",
                    error,
                )
            })?;
        let config_path: OwnedObjectPath =
            device.get_property("Ip4Config").await.map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to read a NetworkManager device IPv4 configuration",
                    error,
                )
            })?;
        if config_path.as_str() == "/" {
            continue;
        }
        let config = Proxy::new(
            &connection,
            NM_SERVICE,
            config_path.as_str(),
            IP4_CONFIG_INTERFACE,
        )
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to access a NetworkManager IPv4 configuration",
                error,
            )
        })?;

        let addresses: Vec<HashMap<String, OwnedValue>> =
            config.get_property("AddressData").await.map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to read NetworkManager active IPv4 address data",
                    error,
                )
            })?;
        for entry in addresses {
            prefixes.push(observed_ipv4_prefix(
                &entry,
                "address",
                "active IPv4 address",
            )?);
        }

        let routes: Vec<HashMap<String, OwnedValue>> =
            config.get_property("RouteData").await.map_err(|error| {
                network_error(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to read NetworkManager active IPv4 route data",
                    error,
                )
            })?;
        for entry in routes {
            prefixes.push(observed_ipv4_prefix(&entry, "dest", "active IPv4 route")?);
        }
    }
    prefixes.sort();
    prefixes.dedup();
    Ok(prefixes)
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
    if proxy.name_has_owner(name).await.map_err(|error| {
        network_error(
            ErrorKind::NetworkBackendUnavailable,
            "failed to query NetworkManager availability",
            error,
        )
    })? {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::NetworkBackendUnavailable,
            "NetworkManager is not available on the system D-Bus",
        ))
    }
}

fn observed_ipv4_prefix(
    entry: &HashMap<String, OwnedValue>,
    address_key: &str,
    context: &str,
) -> Result<IpPrefix, CoreError> {
    let address = IpAddr::from_str(setting_string(entry, address_key)?).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            format!("NetworkManager returned an invalid {context} address"),
            error.to_string(),
        )
    })?;
    if !address.is_ipv4() {
        return Err(CoreError::new(
            ErrorKind::ProtocolError,
            format!("NetworkManager returned non-IPv4 data in {context}"),
        ));
    }
    let prefix = u32::try_from(
        entry
            .get("prefix")
            .ok_or_else(|| {
                CoreError::new(
                    ErrorKind::ProtocolError,
                    format!("NetworkManager {context} entry is missing a prefix"),
                )
            })?
            .clone(),
    )
    .map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            format!("NetworkManager returned an invalid {context} prefix"),
            error.to_string(),
        )
    })?;
    let prefix = u8::try_from(prefix).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            format!("NetworkManager returned an out-of-range {context} prefix"),
            error.to_string(),
        )
    })?;
    normalized_ipv4_prefix(IpPrefix::new(address, prefix)?)
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

fn network_error(
    kind: ErrorKind,
    message: impl Into<String>,
    error: impl std::fmt::Display,
) -> CoreError {
    CoreError::with_diagnostic(kind, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use zbus::zvariant::Str;

    use super::*;

    fn owned_string(value: impl Into<String>) -> OwnedValue {
        OwnedValue::from(Str::from(value.into()))
    }

    #[test]
    fn observed_ipv4_prefixes_are_normalized_and_allow_route_metadata() {
        let address = HashMap::from([
            ("address".to_owned(), owned_string("10.201.44.99")),
            ("prefix".to_owned(), OwnedValue::from(24_u32)),
        ]);
        assert_eq!(
            observed_ipv4_prefix(&address, "address", "active IPv4 address").unwrap(),
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 201, 44, 0)), 24).unwrap()
        );

        let route = HashMap::from([
            ("dest".to_owned(), owned_string("10.202.3.7")),
            ("prefix".to_owned(), OwnedValue::from(24_u32)),
            ("table".to_owned(), OwnedValue::from(254_u32)),
        ]);
        assert_eq!(
            observed_ipv4_prefix(&route, "dest", "active IPv4 route").unwrap(),
            IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 202, 3, 0)), 24).unwrap()
        );
    }

    #[test]
    fn malformed_observed_ipv4_prefix_fails_closed() {
        let entry = HashMap::from([("address".to_owned(), owned_string("10.201.44.99"))]);
        let error = observed_ipv4_prefix(&entry, "address", "active IPv4 address").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ProtocolError);
    }
}

//! Addresses on BlueRoute-owned profiles: reading, ensuring and removing them, and the profile
//! address encoding.

use super::dbus::{network_error, owned_string, setting_string, settings_proxy};
use super::inventory::{connection_from_settings, get_settings};
use super::ownership::{
    exactly_one_owned_profile, matching_profiles, profile_owner, reject_foreign_active_connection,
    validate_interface_name,
};
use super::profile::{
    activate_profile, add_owned_profile, device_by_interface, reapply_if_active, update_profile,
};
use super::{
    APPLY_TIMEOUT, DEVICE_TYPE_BRIDGE, IPV4_SETTING, IPV6_SETTING, KIND_BRIDGE, KIND_INTERFACE,
    NmSettings, RECONCILE_INTERVAL,
};
use crate::InterfaceAddress;
use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind, IpPrefix};
use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Instant;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

pub(super) async fn owned_addresses(
    connection: &Connection,
) -> Result<Vec<InterfaceAddress>, CoreError> {
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

pub(super) async fn ensure_address(
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

pub(super) async fn remove_address(
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

pub(super) fn set_address_prefixes(
    settings: &mut NmSettings,
    prefixes: &[IpPrefix],
) -> Result<(), CoreError> {
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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::super::test_support::{interface, network};
    use super::super::{IPV4_SETTING, KIND_BRIDGE, KIND_INTERFACE};
    use super::*;
    use crate::networkmanager::dbus::setting_string;
    use crate::networkmanager::ownership::base_owned_settings;

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

    /// Settings whose `family` group holds `address-data` built from `entries`.
    fn with_address_data(family: &str, entries: Vec<HashMap<String, OwnedValue>>) -> NmSettings {
        let value = Value::from(entries).try_into_owned().unwrap();
        let mut group = HashMap::new();
        group.insert("address-data".to_owned(), value);
        let mut settings = NmSettings::new();
        settings.insert(family.to_owned(), group);
        settings
    }

    fn address_entry(
        address: Option<OwnedValue>,
        prefix: Option<OwnedValue>,
    ) -> HashMap<String, OwnedValue> {
        let mut entry = HashMap::new();
        if let Some(address) = address {
            entry.insert("address".to_owned(), address);
        }
        if let Some(prefix) = prefix {
            entry.insert("prefix".to_owned(), prefix);
        }
        entry
    }

    #[test]
    fn address_data_rejection_table_fails_closed_with_typed_errors() {
        let text = |value: &str| Some(owned_string(value));
        let number = |value: u32| Some(OwnedValue::from(value));
        let protocol = ErrorKind::ProtocolError;
        let cases = [
            (
                "missing address",
                IPV4_SETTING,
                address_entry(None, number(24)),
                protocol,
            ),
            (
                "non-string address",
                IPV4_SETTING,
                address_entry(number(7), number(24)),
                protocol,
            ),
            (
                "missing prefix",
                IPV4_SETTING,
                address_entry(text("10.42.0.2"), None),
                protocol,
            ),
            (
                "string prefix",
                IPV4_SETTING,
                address_entry(text("10.42.0.2"), text("24")),
                protocol,
            ),
            (
                "invalid address",
                IPV4_SETTING,
                address_entry(text("10.42.0"), number(24)),
                protocol,
            ),
            (
                "prefix 256",
                IPV4_SETTING,
                address_entry(text("10.42.0.2"), number(256)),
                protocol,
            ),
            (
                "IPv4 prefix 33",
                IPV4_SETTING,
                address_entry(text("10.42.0.2"), number(33)),
                ErrorKind::InvalidInput,
            ),
            (
                "IPv6 prefix 129",
                IPV6_SETTING,
                address_entry(text("fd00:42::2"), number(129)),
                ErrorKind::InvalidInput,
            ),
        ];
        for (name, family, entry, kind) in cases {
            let settings = with_address_data(family, vec![entry]);
            let error = address_prefixes(&settings)
                .err()
                .unwrap_or_else(|| panic!("{name}: accepted"));
            assert_eq!(error.kind(), kind, "{name}: {error:?}");
        }

        let mut malformed = NmSettings::new();
        let mut group = HashMap::new();
        group.insert("address-data".to_owned(), OwnedValue::from(true));
        malformed.insert(IPV4_SETTING.to_owned(), group);
        assert_eq!(address_prefixes(&malformed).unwrap_err().kind(), protocol);
    }

    #[test]
    fn address_data_is_optional_and_accepts_boundary_prefixes() {
        let mut settings = NmSettings::new();
        assert!(address_prefixes(&settings).unwrap().is_empty());
        settings.insert(IPV4_SETTING.to_owned(), HashMap::new());
        assert!(address_prefixes(&settings).unwrap().is_empty());

        let host = address_entry(
            Some(owned_string("10.42.0.2")),
            Some(OwnedValue::from(32_u32)),
        );
        let duplicate = address_entry(
            Some(owned_string("10.42.0.2")),
            Some(OwnedValue::from(32_u32)),
        );
        let settings = with_address_data(IPV4_SETTING, vec![host, duplicate]);
        let expected = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 2)), 32).unwrap();
        assert_eq!(address_prefixes(&settings).unwrap(), vec![expected]);

        let v6 = address_entry(
            Some(owned_string("fd00:42::2")),
            Some(OwnedValue::from(128_u32)),
        );
        let settings = with_address_data(IPV6_SETTING, vec![v6]);
        let expected = IpPrefix::new(IpAddr::from_str("fd00:42::2").unwrap(), 128).unwrap();
        assert_eq!(address_prefixes(&settings).unwrap(), vec![expected]);
    }
}

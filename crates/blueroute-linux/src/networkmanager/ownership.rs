//! BlueRoute ownership metadata on NetworkManager profiles: which profiles are BlueRoute's, fail-closed parsing, foreign-claim rejection and deterministic profile identity. Only profiles proven owned here may be changed or deleted.

use super::dbus::{network_error, owned_string, settings_proxy};
use super::inventory::{connection_from_settings, get_settings, list_connections};
use super::profile::device_by_interface;
use super::{
    CONNECTION_SETTING, IPV4_SETTING, IPV6_SETTING, KIND_BRIDGE, KIND_INTERFACE, KIND_KEY,
    NmSettings, OWNER_KEY, OWNERSHIP_SCHEMA, SCHEMA_KEY, USER_DATA_PROPERTY, USER_SETTING,
};
use crate::{NetworkConnection, NetworkDevice, NetworkInterfaceHandle};
use blueroute_core::{CoreError, ErrorKind, NetworkId};
use std::collections::HashMap;
use std::str::FromStr;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

pub(super) struct OwnedProfile {
    pub(super) path: String,
    pub(super) connection: NetworkConnection,
    pub(super) settings: NmSettings,
}

pub(super) async fn matching_profiles(
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

pub(super) fn exactly_one_owned_profile(
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

pub(super) async fn reject_foreign_interface_claim(
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

pub(super) async fn reject_foreign_active_connection(
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

pub(super) fn base_owned_settings(
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

pub(super) fn profile_owner(settings: &NmSettings) -> Result<Option<NetworkId>, CoreError> {
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

pub(super) fn validate_interface_name(interface: &NetworkInterfaceHandle) -> Result<(), CoreError> {
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

#[cfg(test)]
mod tests {

    use super::super::test_support::{interface, network};
    use super::super::{
        CONNECTION_SETTING, IPV4_SETTING, IPV6_SETTING, KIND_BRIDGE, KIND_INTERFACE, KIND_KEY,
        OWNER_KEY, OWNERSHIP_SCHEMA, SCHEMA_KEY, USER_DATA_PROPERTY, USER_SETTING,
    };
    use super::*;
    use crate::NetworkConnectionHandle;
    use crate::networkmanager::dbus::setting_string;

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
    fn overlong_interface_name_is_rejected_before_dbus() {
        let error = validate_interface_name(&interface("abcdefghijklmnop")).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

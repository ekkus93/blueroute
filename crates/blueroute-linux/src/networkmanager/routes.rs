//! Routes on BlueRoute-owned profiles: reading, ensuring and removing them, validation, and the
//! profile route encoding.

use super::dbus::{
    network_error, optional_setting_string, owned_string, setting_string, settings_proxy,
};
use super::inventory::{connection_from_settings, get_settings};
use super::ownership::{
    exactly_one_owned_profile, matching_profiles, profile_owner, validate_interface_name,
};
use super::profile::{activate_profile, reapply_if_active, update_profile};
use super::{APPLY_TIMEOUT, IPV4_SETTING, IPV6_SETTING, NmSettings, RECONCILE_INTERVAL};
use crate::LinuxRoute;
use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind, IpPrefix};
use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Instant;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

pub(super) async fn owned_routes(connection: &Connection) -> Result<Vec<LinuxRoute>, CoreError> {
    let paths: Vec<OwnedObjectPath> = settings_proxy(connection)
        .await?
        .call("ListConnections", &())
        .await
        .map_err(|error| {
            network_error(
                ErrorKind::NetworkBackendUnavailable,
                "failed to enumerate NetworkManager connection profiles for routes",
                error,
            )
        })?;
    let mut routes = Vec::new();
    for path in paths {
        let settings = get_settings(connection, path.as_str()).await?;
        let Some(owner) = profile_owner(&settings)? else {
            continue;
        };
        let profile = connection_from_settings(path.as_str(), &settings)?;
        let Some(interface) = profile.interface else {
            continue;
        };
        for (family, ipv4) in [(IPV4_SETTING, true), (IPV6_SETTING, false)] {
            for entry in route_data_entries(&settings, family)? {
                let (destination, via, metric) = route_key_from_entry(&entry, ipv4)?;
                routes.push(LinuxRoute {
                    destination,
                    via,
                    interface: interface.clone(),
                    metric,
                    owner,
                });
            }
        }
    }
    routes.sort_by(|left, right| {
        left.interface
            .cmp(&right.interface)
            .then_with(|| left.owner.cmp(&right.owner))
            .then_with(|| left.destination.cmp(&right.destination))
            .then_with(|| left.via.cmp(&right.via))
            .then_with(|| left.metric.cmp(&right.metric))
    });
    routes.dedup();
    Ok(routes)
}

pub(super) async fn ensure_route(
    connection: &Connection,
    route: &LinuxRoute,
) -> Result<(), CoreError> {
    validate_linux_route(route)?;
    let profiles = matching_profiles(connection, route.owner, &route.interface).await?;
    let profile =
        exactly_one_owned_profile(profiles, route.owner, &route.interface)?.ok_or_else(|| {
            CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "cannot add a route without an existing BlueRoute-owned NetworkManager profile",
                format!(
                    "owner={} interface={}",
                    route.owner,
                    route.interface.as_str()
                ),
            )
        })?;
    let mut settings = profile.settings;
    if profile_owner(&settings)? != Some(route.owner) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "refusing to modify routes on a NetworkManager profile not owned by the requested BlueRoute network",
        ));
    }
    let changed = set_route_present(&mut settings, route, true)?;
    if changed {
        update_profile(connection, &profile.path, &settings).await?;
    }
    activate_profile(connection, &profile.path, &route.interface).await?;
    reapply_if_active(connection, &profile.path, &route.interface, &settings).await?;
    wait_for_route(connection, route, true).await
}

pub(super) async fn remove_route(
    connection: &Connection,
    route: &LinuxRoute,
) -> Result<(), CoreError> {
    validate_linux_route(route)?;
    let profiles = matching_profiles(connection, route.owner, &route.interface).await?;
    let Some(profile) = exactly_one_owned_profile(profiles, route.owner, &route.interface)? else {
        return Ok(());
    };
    let mut settings = profile.settings;
    if profile_owner(&settings)? != Some(route.owner) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "refusing to remove routes from a NetworkManager profile not owned by the requested BlueRoute network",
        ));
    }
    if !set_route_present(&mut settings, route, false)? {
        return Ok(());
    }
    update_profile(connection, &profile.path, &settings).await?;
    reapply_if_active(connection, &profile.path, &route.interface, &settings).await?;
    wait_for_route(connection, route, false).await
}

async fn wait_for_route(
    connection: &Connection,
    route: &LinuxRoute,
    present: bool,
) -> Result<(), CoreError> {
    let deadline = Instant::now() + APPLY_TIMEOUT;
    loop {
        let current = owned_routes(connection).await?;
        let found = current.contains(route);
        if found == present {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                if present {
                    "NetworkManager did not record the BlueRoute route before the timeout"
                } else {
                    "NetworkManager did not remove the BlueRoute route before the timeout"
                },
                format!(
                    "owner={} interface={} destination={:?} via={:?} metric={}",
                    route.owner,
                    route.interface.as_str(),
                    route.destination,
                    route.via,
                    route.metric
                ),
            ));
        }
        Timer::after(RECONCILE_INTERVAL).await;
    }
}

fn validate_linux_route(route: &LinuxRoute) -> Result<(), CoreError> {
    validate_interface_name(&route.interface)?;
    if let Some(via) = route.via
        && via.is_ipv4() != route.destination.address.is_ipv4()
    {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidInput,
            "route destination and next hop must use the same IP family",
            format!("destination={:?} via={via}", route.destination),
        ));
    }
    Ok(())
}

fn route_data_entries(
    settings: &NmSettings,
    family: &str,
) -> Result<Vec<HashMap<String, OwnedValue>>, CoreError> {
    let Some(group) = settings.get(family) else {
        return Ok(Vec::new());
    };
    let Some(value) = group.get("route-data") else {
        return Ok(Vec::new());
    };
    Vec::<HashMap<String, OwnedValue>>::try_from(value.clone()).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned malformed route-data",
            error.to_string(),
        )
    })
}

fn route_key_from_entry(
    entry: &HashMap<String, OwnedValue>,
    ipv4: bool,
) -> Result<(IpPrefix, Option<IpAddr>, u32), CoreError> {
    for key in entry.keys() {
        if !matches!(key.as_str(), "dest" | "prefix" | "next-hop" | "metric") {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "NetworkManager BlueRoute-owned route-data contains unsupported route attributes",
                key,
            ));
        }
    }
    let destination = IpAddr::from_str(setting_string(entry, "dest")?).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned an invalid route destination",
            error.to_string(),
        )
    })?;
    if destination.is_ipv4() != ipv4 {
        return Err(CoreError::new(
            ErrorKind::ProtocolError,
            "NetworkManager route-data destination is stored under the wrong IP family",
        ));
    }
    let prefix = u32::try_from(
        entry
            .get("prefix")
            .ok_or_else(|| {
                CoreError::new(
                    ErrorKind::ProtocolError,
                    "NetworkManager route-data entry is missing a prefix",
                )
            })?
            .clone(),
    )
    .map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned an invalid route prefix",
            error.to_string(),
        )
    })?;
    let prefix = u8::try_from(prefix).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned an out-of-range route prefix length",
            error.to_string(),
        )
    })?;
    let destination = IpPrefix::new(destination, prefix)?;
    let via = optional_setting_string(entry, "next-hop")?
        .filter(|value| !value.is_empty())
        .map(IpAddr::from_str)
        .transpose()
        .map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::ProtocolError,
                "NetworkManager returned an invalid route next hop",
                error.to_string(),
            )
        })?;
    if let Some(via) = via
        && via.is_ipv4() != ipv4
    {
        return Err(CoreError::new(
            ErrorKind::ProtocolError,
            "NetworkManager route-data next hop uses the wrong IP family",
        ));
    }
    let metric = u32::try_from(
        entry
            .get("metric")
            .ok_or_else(|| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "BlueRoute-owned NetworkManager route-data entry is missing an explicit metric",
                )
            })?
            .clone(),
    )
    .map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "NetworkManager returned an invalid route metric",
            error.to_string(),
        )
    })?;
    Ok((destination, via, metric))
}

fn encode_route_entry(route: &LinuxRoute) -> HashMap<String, OwnedValue> {
    let mut entry = HashMap::new();
    entry.insert(
        "dest".to_owned(),
        owned_string(route.destination.address.to_string()),
    );
    entry.insert(
        "prefix".to_owned(),
        OwnedValue::from(u32::from(route.destination.prefix_len)),
    );
    if let Some(via) = route.via {
        entry.insert("next-hop".to_owned(), owned_string(via.to_string()));
    }
    entry.insert("metric".to_owned(), OwnedValue::from(route.metric));
    entry
}

fn set_route_present(
    settings: &mut NmSettings,
    route: &LinuxRoute,
    present: bool,
) -> Result<bool, CoreError> {
    let ipv4 = route.destination.address.is_ipv4();
    let family = if ipv4 { IPV4_SETTING } else { IPV6_SETTING };
    if present {
        let group = settings.get(family).ok_or_else(|| {
            CoreError::new(
                ErrorKind::InvalidState,
                "BlueRoute-owned NetworkManager profile is missing IP settings for the route family",
            )
        })?;
        if setting_string(group, "method")? != "manual" {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "BlueRoute routes require the target IP family to be configured manually before adding routes",
                format!("interface={} family={family}", route.interface.as_str()),
            ));
        }
    }

    let mut entries = route_data_entries(settings, family)?;
    let target = (route.destination, route.via, route.metric);
    let mut changed = false;
    let mut found = false;
    let mut kept = Vec::with_capacity(entries.len() + if present { 1 } else { 0 });
    for entry in entries.drain(..) {
        let key = route_key_from_entry(&entry, ipv4)?;
        if present && key.0 == route.destination {
            if key == target && !found {
                found = true;
                kept.push(entry);
            } else {
                // Destination is the route identity for the current LinuxRoute model.
                // Reconcile stale next-hop/metric variants instead of accumulating them.
                changed = true;
            }
        } else if !present && key == target {
            changed = true;
        } else {
            kept.push(entry);
        }
    }
    if present && !found {
        kept.push(encode_route_entry(route));
        changed = true;
    }

    let group = settings.entry(family.to_owned()).or_default();
    if group.remove("routes").is_some() {
        changed = true;
    }
    if changed {
        if kept.is_empty() {
            group.remove("route-data");
        } else {
            let route_data = Value::from(kept).try_into_owned().map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "failed to encode NetworkManager route-data",
                    error.to_string(),
                )
            })?;
            group.insert("route-data".to_owned(), route_data);
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::super::test_support::{interface, network};
    use super::super::{IPV4_SETTING, KIND_BRIDGE};
    use super::*;
    use crate::networkmanager::addresses::set_address_prefixes;
    use crate::networkmanager::ownership::base_owned_settings;

    #[test]
    fn route_mutation_round_trips_is_idempotent_and_preserves_unrelated_routes() {
        let owner = network(6);
        let iface = interface("br-blue0");
        let mut settings = base_owned_settings(owner, &iface, KIND_BRIDGE).unwrap();
        let address = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 1)), 24).unwrap();
        set_address_prefixes(&mut settings, &[address]).unwrap();
        let first = LinuxRoute {
            destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 43, 0, 0)), 24).unwrap(),
            via: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 2))),
            interface: iface.clone(),
            metric: 77,
            owner,
        };
        let second = LinuxRoute {
            destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 44, 0, 0)), 24).unwrap(),
            via: None,
            interface: iface,
            metric: 88,
            owner,
        };
        assert!(set_route_present(&mut settings, &first, true).unwrap());
        assert!(!set_route_present(&mut settings, &first, true).unwrap());
        assert!(set_route_present(&mut settings, &second, true).unwrap());
        let entries = route_data_entries(&settings, IPV4_SETTING).unwrap();
        assert_eq!(entries.len(), 2);

        let updated = LinuxRoute {
            destination: first.destination,
            via: None,
            interface: first.interface.clone(),
            metric: 99,
            owner,
        };
        assert!(set_route_present(&mut settings, &updated, true).unwrap());
        assert!(!set_route_present(&mut settings, &updated, true).unwrap());
        let entries = route_data_entries(&settings, IPV4_SETTING).unwrap();
        assert_eq!(entries.len(), 2);
        let keys: Vec<_> = entries
            .iter()
            .map(|entry| route_key_from_entry(entry, true).unwrap())
            .collect();
        assert!(keys.contains(&(updated.destination, updated.via, updated.metric)));
        assert!(keys.contains(&(second.destination, second.via, second.metric)));
        assert!(!keys.contains(&(first.destination, first.via, first.metric)));

        assert!(set_route_present(&mut settings, &updated, false).unwrap());
        let entries = route_data_entries(&settings, IPV4_SETTING).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            route_key_from_entry(&entries[0], true).unwrap(),
            (second.destination, second.via, second.metric)
        );
        assert!(!set_route_present(&mut settings, &updated, false).unwrap());
    }

    #[test]
    fn route_mutation_removes_legacy_routes_shadow() {
        let owner = network(10);
        let iface = interface("br-blue0");
        let mut settings = base_owned_settings(owner, &iface, KIND_BRIDGE).unwrap();
        let address = IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 1)), 24).unwrap();
        set_address_prefixes(&mut settings, &[address]).unwrap();
        settings
            .get_mut(IPV4_SETTING)
            .unwrap()
            .insert("routes".to_owned(), OwnedValue::from(true));
        let route = LinuxRoute {
            destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 43, 0, 0)), 24).unwrap(),
            via: None,
            interface: iface,
            metric: 77,
            owner,
        };
        assert!(set_route_present(&mut settings, &route, true).unwrap());
        assert!(!settings.get(IPV4_SETTING).unwrap().contains_key("routes"));
        assert_eq!(
            route_data_entries(&settings, IPV4_SETTING).unwrap().len(),
            1
        );
    }

    #[test]
    fn route_validation_rejects_cross_family_next_hop() {
        let route = LinuxRoute {
            destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 43, 0, 0)), 24).unwrap(),
            via: Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            interface: interface("br-blue0"),
            metric: 77,
            owner: network(6),
        };
        assert_eq!(
            validate_linux_route(&route).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn unsupported_route_attributes_fail_closed() {
        let owner = network(11);
        let route = LinuxRoute {
            destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 43, 0, 0)), 24).unwrap(),
            via: None,
            interface: interface("br-blue0"),
            metric: 77,
            owner,
        };
        let mut entry = encode_route_entry(&route);
        entry.insert("table".to_owned(), OwnedValue::from(254_u32));
        assert_eq!(
            route_key_from_entry(&entry, true).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
    }
}

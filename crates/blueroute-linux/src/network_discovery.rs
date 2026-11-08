use std::collections::HashMap;

use zbus::Proxy;
use zbus::fdo::{ManagedObjects, ObjectManagerProxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use blueroute_core::{CoreError, ErrorKind, NetworkId};

use crate::{AdapterHandle, BluetoothBackend, BluezBackend, PeerHandle};

const BLUEZ_SERVICE: &str = "org.bluez";
const BLUEZ_ROOT_PATH: &str = "/";
const DEVICE_INTERFACE: &str = "org.bluez.Device1";
const LE_ADVERTISING_MANAGER_INTERFACE: &str = "org.bluez.LEAdvertisingManager1";
const MANUFACTURER_DATA_PROPERTY: &str = "ManufacturerData";
const REGISTER_ADVERTISEMENT_METHOD: &str = "RegisterAdvertisement";
const UNREGISTER_ADVERTISEMENT_METHOD: &str = "UnregisterAdvertisement";
const BLUEROUTE_ADVERTISEMENT_PATH: &str = "/org/blueroute/NetworkAdvertisement";
const BLUEROUTE_MANUFACTURER_ID: u16 = 0xffff;
const BLUEROUTE_ADVERTISEMENT_MAGIC: [u8; 2] = *b"BR";
const BLUEROUTE_ADVERTISEMENT_VERSION: u8 = 1;
const BLUEROUTE_ADVERTISEMENT_ROLE_NAP: u8 = 0x01;
const BLUEROUTE_ADVERTISEMENT_PAYLOAD_LEN: usize = 20;

/// Opaque handle for one BlueRoute network advertisement registered with BlueZ.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkAdvertisement {
    adapter: AdapterHandle,
    network: NetworkId,
}

impl NetworkAdvertisement {
    pub fn adapter(&self) -> &AdapterHandle {
        &self.adapter
    }

    pub const fn network(&self) -> NetworkId {
        self.network
    }
}

#[derive(Clone, Debug)]
struct BlueRouteAdvertisement {
    manufacturer_data: HashMap<u16, OwnedValue>,
}

impl BlueRouteAdvertisement {
    fn new(network: NetworkId) -> Result<Self, CoreError> {
        let payload = encode_advertisement(network);
        let value = OwnedValue::try_from(Value::from(payload)).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "failed to encode BlueRoute discovery advertisement",
                error.to_string(),
            )
        })?;
        Ok(Self {
            manufacturer_data: HashMap::from([(BLUEROUTE_MANUFACTURER_ID, value)]),
        })
    }
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl BlueRouteAdvertisement {
    fn release(&self) {}

    #[zbus(property, name = "Type")]
    fn advertisement_type(&self) -> &str {
        "peripheral"
    }

    #[zbus(property, name = "Discoverable")]
    fn discoverable(&self) -> bool {
        true
    }

    #[zbus(property, name = "ManufacturerData")]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedValue> {
        self.manufacturer_data.clone()
    }
}

impl BluezBackend {
    /// Advertises one hosted BlueRoute network through BlueZ LE manufacturer data.
    ///
    /// The advertised network ID is a discovery hint, not authenticated membership proof.
    pub async fn start_network_advertisement(
        &self,
        adapter: AdapterHandle,
        network: NetworkId,
    ) -> Result<NetworkAdvertisement, CoreError> {
        let objects = managed_objects(self).await?;
        ensure_advertising_manager(self, &objects, &adapter).await?;
        let manager = advertising_manager_proxy(self, &adapter).await?;
        let advertisement = BlueRouteAdvertisement::new(network)?;
        let added = self
            .connection
            .object_server()
            .at(BLUEROUTE_ADVERTISEMENT_PATH, advertisement)
            .await
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "failed to serve the BlueRoute network advertisement",
                    error.to_string(),
                )
            })?;
        if !added {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "a BlueRoute network advertisement is already active on this D-Bus connection",
            ));
        }

        let path = advertisement_path()?;
        let options = HashMap::<String, OwnedValue>::new();
        if let Err(error) = manager
            .call_method(REGISTER_ADVERTISEMENT_METHOD, &(path, options))
            .await
        {
            let registration_error =
                advertisement_method_error(REGISTER_ADVERTISEMENT_METHOD, error);
            return match self
                .connection
                .object_server()
                .remove::<BlueRouteAdvertisement, _>(BLUEROUTE_ADVERTISEMENT_PATH)
                .await
            {
                Ok(_) => Err(registration_error),
                Err(cleanup_error) => Err(CoreError::with_diagnostic(
                    registration_error.kind(),
                    registration_error.message(),
                    format!(
                        "{}; failed to remove rejected advertisement object: {cleanup_error}",
                        registration_error
                            .diagnostic()
                            .unwrap_or("no registration diagnostic")
                    ),
                )),
            };
        }

        Ok(NetworkAdvertisement { adapter, network })
    }

    /// Stops a previously registered BlueRoute network advertisement.
    pub async fn stop_network_advertisement(
        &self,
        advertisement: NetworkAdvertisement,
    ) -> Result<(), CoreError> {
        let manager = advertising_manager_proxy(self, &advertisement.adapter).await?;
        let path = advertisement_path()?;
        match manager
            .call_method(UNREGISTER_ADVERTISEMENT_METHOD, &(path,))
            .await
        {
            Ok(_) => {}
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.bluez.Error.DoesNotExist" => {}
            Err(error) => {
                return Err(advertisement_method_error(
                    UNREGISTER_ADVERTISEMENT_METHOD,
                    error,
                ));
            }
        }

        self.connection
            .object_server()
            .remove::<BlueRouteAdvertisement, _>(BLUEROUTE_ADVERTISEMENT_PATH)
            .await
            .map(|_| ())
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "failed to remove the BlueRoute network advertisement object",
                    error.to_string(),
                )
            })
    }

    /// Returns valid BlueRoute network IDs currently visible under one BlueZ adapter.
    ///
    /// These IDs are unauthenticated discovery metadata. Malformed peer-controlled records are
    /// ignored instead of making the entire discovery snapshot fail.
    pub async fn discovered_network_ids(
        &self,
        adapter: AdapterHandle,
    ) -> Result<Vec<NetworkId>, CoreError> {
        let mut networks: Vec<NetworkId> = self
            .advertising_devices(adapter)
            .await?
            .into_iter()
            .map(|(_, network)| network)
            .collect();
        networks.sort_unstable();
        networks.dedup();
        Ok(networks)
    }

    /// BlueZ devices under one adapter that currently advertise `network`.
    ///
    /// This is an **untrusted locator hint**: an advertisement is not authenticated, so a
    /// returned device may be an impostor or a stale cache entry. The caller must authenticate
    /// whatever it connects to (pinned mutual TLS) and must fail closed on zero or several
    /// candidates instead of choosing one.
    pub async fn discovered_network_peers(
        &self,
        adapter: AdapterHandle,
        network: NetworkId,
    ) -> Result<Vec<PeerHandle>, CoreError> {
        let mut peers: Vec<PeerHandle> = self
            .advertising_devices(adapter)
            .await?
            .into_iter()
            .filter(|(_, advertised)| *advertised == network)
            .map(|(peer, _)| peer)
            .collect();
        peers.sort();
        peers.dedup();
        Ok(peers)
    }

    async fn advertising_devices(
        &self,
        adapter: AdapterHandle,
    ) -> Result<Vec<(PeerHandle, NetworkId)>, CoreError> {
        let objects = managed_objects(self).await?;
        ensure_adapter_present(self, &adapter).await?;
        advertising_devices_in(&objects, &adapter)
    }
}

fn advertising_devices_in(
    objects: &ManagedObjects,
    adapter: &AdapterHandle,
) -> Result<Vec<(PeerHandle, NetworkId)>, CoreError> {
    let mut devices = Vec::new();
    for (path, interfaces) in objects {
        if !is_device_object_path_for_adapter(path.as_str(), adapter) {
            continue;
        }
        let Some(properties) = interfaces.iter().find_map(|(name, properties)| {
            (name.as_str() == DEVICE_INTERFACE).then_some(properties)
        }) else {
            continue;
        };
        if let Some(network) = advertised_network_id(properties) {
            devices.push((PeerHandle::new(path.as_str())?, network));
        }
    }
    Ok(devices)
}

fn encode_advertisement(network: NetworkId) -> Vec<u8> {
    let mut payload = Vec::with_capacity(BLUEROUTE_ADVERTISEMENT_PAYLOAD_LEN);
    payload.extend_from_slice(&BLUEROUTE_ADVERTISEMENT_MAGIC);
    payload.push(BLUEROUTE_ADVERTISEMENT_VERSION);
    payload.push(BLUEROUTE_ADVERTISEMENT_ROLE_NAP);
    payload.extend_from_slice(network.as_bytes());
    payload
}

fn decode_advertisement(payload: &[u8]) -> Option<NetworkId> {
    if payload.len() != BLUEROUTE_ADVERTISEMENT_PAYLOAD_LEN
        || payload[..2] != BLUEROUTE_ADVERTISEMENT_MAGIC
        || payload[2] != BLUEROUTE_ADVERTISEMENT_VERSION
        || payload[3] & BLUEROUTE_ADVERTISEMENT_ROLE_NAP == 0
    {
        return None;
    }

    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&payload[4..]);
    Some(NetworkId::from_bytes(bytes))
}

fn advertised_network_id(properties: &HashMap<String, OwnedValue>) -> Option<NetworkId> {
    let manufacturer = properties
        .get(MANUFACTURER_DATA_PROPERTY)?
        .try_clone()
        .ok()?;
    let manufacturer = HashMap::<u16, OwnedValue>::try_from(manufacturer).ok()?;
    let payload = manufacturer
        .get(&BLUEROUTE_MANUFACTURER_ID)?
        .try_clone()
        .ok()?;
    let payload = Vec::<u8>::try_from(payload).ok()?;
    decode_advertisement(&payload)
}

async fn managed_objects(backend: &BluezBackend) -> Result<ManagedObjects, CoreError> {
    if !backend.service_available().await? {
        return Err(CoreError::new(
            ErrorKind::BluezUnavailable,
            "BlueZ is not available on the system D-Bus",
        ));
    }
    let proxy = ObjectManagerProxy::new(&backend.connection, BLUEZ_SERVICE, BLUEZ_ROOT_PATH)
        .await
        .map_err(|error| bluez_error("failed to create the BlueZ object-manager proxy", error))?;
    proxy
        .get_managed_objects()
        .await
        .map_err(|error| bluez_error("failed to enumerate BlueZ managed objects", error))
}

async fn ensure_adapter_present(
    backend: &BluezBackend,
    adapter: &AdapterHandle,
) -> Result<(), CoreError> {
    if backend
        .adapters()
        .await?
        .into_iter()
        .any(|candidate| candidate.handle == *adapter)
    {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::MissingAdapter,
            "Bluetooth adapter is no longer available",
        ))
    }
}

async fn ensure_advertising_manager(
    backend: &BluezBackend,
    objects: &ManagedObjects,
    adapter: &AdapterHandle,
) -> Result<(), CoreError> {
    let current = backend
        .adapters()
        .await?
        .into_iter()
        .find(|candidate| candidate.handle == *adapter)
        .ok_or_else(|| {
            CoreError::new(
                ErrorKind::MissingAdapter,
                "Bluetooth adapter is no longer available",
            )
        })?;
    if !current.powered {
        return Err(CoreError::new(
            ErrorKind::AdapterDisabled,
            "Bluetooth adapter must be powered before BlueRoute discovery advertising can start",
        ));
    }

    let path = OwnedObjectPath::try_from(adapter.as_str()).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::MissingAdapter,
            "Bluetooth adapter object path is invalid",
            error.to_string(),
        )
    })?;
    let Some(interfaces) = objects.get(&path) else {
        return Err(CoreError::new(
            ErrorKind::MissingAdapter,
            "Bluetooth adapter is no longer available",
        ));
    };
    if interfaces
        .keys()
        .any(|name| name.as_str() == LE_ADVERTISING_MANAGER_INTERFACE)
    {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::CapabilityUnavailable,
            "Bluetooth adapter does not expose BlueZ LE advertising capability",
        ))
    }
}

async fn advertising_manager_proxy<'a>(
    backend: &'a BluezBackend,
    adapter: &'a AdapterHandle,
) -> Result<Proxy<'a>, CoreError> {
    Proxy::new(
        &backend.connection,
        BLUEZ_SERVICE,
        adapter.as_str(),
        LE_ADVERTISING_MANAGER_INTERFACE,
    )
    .await
    .map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::CapabilityUnavailable,
            "failed to create the BlueZ LE advertising-manager proxy",
            error.to_string(),
        )
    })
}

fn advertisement_path() -> Result<OwnedObjectPath, CoreError> {
    OwnedObjectPath::try_from(BLUEROUTE_ADVERTISEMENT_PATH).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::Internal,
            "BlueRoute advertisement object path is invalid",
            error.to_string(),
        )
    })
}

fn advertisement_method_error(method: &'static str, error: zbus::Error) -> CoreError {
    let kind = match &error {
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "org.bluez.Error.NotReady" => ErrorKind::AdapterDisabled,
            "org.bluez.Error.AlreadyExists" => ErrorKind::InvalidState,
            "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject" => {
                ErrorKind::MissingAdapter
            }
            "org.bluez.Error.InvalidLength"
            | "org.bluez.Error.NotPermitted"
            | "org.bluez.Error.NotSupported"
            | "org.bluez.Error.InvalidArguments" => ErrorKind::CapabilityUnavailable,
            _ => ErrorKind::CapabilityUnavailable,
        },
        _ => ErrorKind::BluezUnavailable,
    };
    let operation = if method == REGISTER_ADVERTISEMENT_METHOD {
        "register the BlueRoute discovery advertisement"
    } else {
        "unregister the BlueRoute discovery advertisement"
    };
    CoreError::with_diagnostic(kind, format!("failed to {operation}"), error.to_string())
}

fn bluez_error(context: &'static str, error: impl std::fmt::Display) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::BluezUnavailable, context, error.to_string())
}

fn is_device_object_path_for_adapter(path: &str, adapter: &AdapterHandle) -> bool {
    let prefix = format!("{}/dev_", adapter.as_str().trim_end_matches('/'));
    path.strip_prefix(&prefix)
        .is_some_and(|suffix| !suffix.is_empty() && !suffix.contains('/'))
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::Str;

    use super::*;

    fn manufacturer_data_value(network: NetworkId) -> OwnedValue {
        let advertisement = BlueRouteAdvertisement::new(network).unwrap();
        OwnedValue::from(advertisement.manufacturer_data)
    }

    #[test]
    fn advertisement_round_trips_full_network_identity() {
        let network = NetworkId::from_bytes([0x5a; 16]);
        let payload = encode_advertisement(network);
        assert_eq!(payload.len(), BLUEROUTE_ADVERTISEMENT_PAYLOAD_LEN);
        assert_eq!(decode_advertisement(&payload), Some(network));
    }

    #[test]
    fn malformed_or_non_nap_advertisements_are_not_candidates() {
        let network = NetworkId::from_bytes([0x31; 16]);
        let mut payload = encode_advertisement(network);
        payload[0] = b'X';
        assert_eq!(decode_advertisement(&payload), None);

        let mut payload = encode_advertisement(network);
        payload[2] = BLUEROUTE_ADVERTISEMENT_VERSION + 1;
        assert_eq!(decode_advertisement(&payload), None);

        let mut payload = encode_advertisement(network);
        payload[3] = 0;
        assert_eq!(decode_advertisement(&payload), None);
        assert_eq!(decode_advertisement(&payload[..19]), None);
    }

    #[test]
    fn advertised_identity_is_independent_of_bluetooth_name() {
        let network = NetworkId::from_bytes([0x42; 16]);
        let mut properties = HashMap::from([
            (
                MANUFACTURER_DATA_PROPERTY.to_owned(),
                manufacturer_data_value(network),
            ),
            (
                "Alias".to_owned(),
                OwnedValue::from(Str::from("Spoofable friendly name")),
            ),
        ]);
        assert_eq!(advertised_network_id(&properties), Some(network));

        properties.insert(
            "Alias".to_owned(),
            OwnedValue::from(Str::from("Completely different name")),
        );
        assert_eq!(advertised_network_id(&properties), Some(network));
    }

    #[test]
    fn malformed_manufacturer_data_is_ignored() {
        let properties = HashMap::from([(
            MANUFACTURER_DATA_PROPERTY.to_owned(),
            OwnedValue::from(true),
        )]);
        assert_eq!(advertised_network_id(&properties), None);
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
    }

    #[test]
    fn peers_are_selected_by_advertised_network_not_by_name() {
        use zbus::names::OwnedInterfaceName;
        use zbus::zvariant::OwnedObjectPath;
        let wanted = NetworkId::from_bytes([0x11; 16]);
        let other = NetworkId::from_bytes([0x22; 16]);
        let device = |network: Option<NetworkId>| {
            let mut properties = HashMap::from([(
                "Alias".to_owned(),
                OwnedValue::from(Str::from("Same friendly name")),
            )]);
            if let Some(network) = network {
                properties.insert(
                    MANUFACTURER_DATA_PROPERTY.to_owned(),
                    manufacturer_data_value(network),
                );
            }
            HashMap::from([(
                OwnedInterfaceName::try_from(DEVICE_INTERFACE).unwrap(),
                properties,
            )])
        };
        let path = |text: &str| OwnedObjectPath::try_from(text).unwrap();
        let objects: ManagedObjects = HashMap::from([
            (
                path("/org/bluez/hci0/dev_AA_00_00_00_00_01"),
                device(Some(wanted)),
            ),
            (
                path("/org/bluez/hci0/dev_AA_00_00_00_00_02"),
                device(Some(other)),
            ),
            (path("/org/bluez/hci0/dev_AA_00_00_00_00_03"), device(None)),
            (
                path("/org/bluez/hci1/dev_AA_00_00_00_00_04"),
                device(Some(wanted)),
            ),
        ]);
        let hci0 = AdapterHandle::new("/org/bluez/hci0").unwrap();
        let found = advertising_devices_in(&objects, &hci0).unwrap();
        let mut matching: Vec<_> = found
            .iter()
            .filter(|(_, network)| *network == wanted)
            .map(|(peer, _)| peer.as_str().to_owned())
            .collect();
        matching.sort();
        // Only the device on this adapter that advertises the network; other adapters, other
        // networks and devices without an advertisement are excluded.
        assert_eq!(matching, vec!["/org/bluez/hci0/dev_AA_00_00_00_00_01"]);
        assert_eq!(found.len(), 2);
    }
}

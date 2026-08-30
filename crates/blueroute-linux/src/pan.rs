use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Duration;

use async_io::Timer;
use futures_lite::{StreamExt, future};
use zbus::message::Type as MessageType;
use zbus::names::OwnedInterfaceName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, Message, MessageStream, Proxy};

use blueroute_core::{CoreError, ErrorKind};

use crate::{
    AdapterHandle, BackendFuture, BluezBackend, NetworkInterfaceHandle, PanAttachment, PanBackend,
    PanRole, PanuEvent, PanuEventSubscription, PeerHandle,
};

const BLUEZ_SERVICE: &str = "org.bluez";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const NETWORK_INTERFACE: &str = "org.bluez.Network1";
const DEVICE_INTERFACE: &str = "org.bluez.Device1";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const INTERFACES_REMOVED: &str = "InterfacesRemoved";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const CONNECT_METHOD: &str = "Connect";
const DISCONNECT_METHOD: &str = "Disconnect";
const CONNECTED_PROPERTY: &str = "Connected";
const INTERFACE_PROPERTY: &str = "Interface";
const ADDRESS_PROPERTY: &str = "Address";
const REMOTE_NAP_ROLE: &str = "nap";
const SIGNAL_QUEUE_CAPACITY: usize = 64;
const PANU_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const PANU_INTERFACE_SETTLE_DELAY: Duration = Duration::from_millis(300);
const SYS_CLASS_NET: &str = "/sys/class/net";

impl PanBackend for BluezBackend {
    fn connect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, PanAttachment> {
        Box::pin(async move { connect_panu(&self.connection, &peer).await })
    }

    fn disconnect_panu(&self, peer: PeerHandle) -> BackendFuture<'_, ()> {
        Box::pin(async move { disconnect_panu(&self.connection, &peer).await })
    }

    fn subscribe_panu_events(
        &self,
        attachment: PanAttachment,
    ) -> BackendFuture<'_, Box<dyn PanuEventSubscription>> {
        Box::pin(async move {
            validate_panu_attachment(&attachment)?;
            // Subscribe before taking the state snapshot so a loss cannot fall into a
            // check-then-subscribe race window.
            let stream = bluez_signal_stream(&self.connection).await?;
            let connected =
                current_panu_attachment(&self.connection, attachment.peer.as_ref().unwrap())
                    .await?
                    .is_some();
            Ok(Box::new(BluezPanuSubscription {
                connection: self.connection.clone(),
                stream,
                attachment,
                pending_loss: !connected,
                finished: false,
            }) as Box<dyn PanuEventSubscription>)
        })
    }

    fn start_nap(&self, _adapter: AdapterHandle) -> BackendFuture<'_, PanAttachment> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "BlueZ NAP lifecycle is not implemented yet",
            ))
        })
    }

    fn stop_nap(&self, _adapter: AdapterHandle) -> BackendFuture<'_, ()> {
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "BlueZ NAP lifecycle is not implemented yet",
            ))
        })
    }
}

struct BluezPanuSubscription {
    connection: Connection,
    stream: MessageStream,
    attachment: PanAttachment,
    pending_loss: bool,
    finished: bool,
}

impl PanuEventSubscription for BluezPanuSubscription {
    fn next_event(&mut self) -> BackendFuture<'_, Option<PanuEvent>> {
        Box::pin(async move {
            if self.finished {
                return Ok(None);
            }
            if self.pending_loss {
                self.pending_loss = false;
                self.finished = true;
                return Ok(Some(PanuEvent::Lost(self.attachment.clone())));
            }

            let peer = self
                .attachment
                .peer
                .as_ref()
                .expect("validated PANU attachment must have a peer")
                .clone();
            loop {
                let Some(message) = self.stream.next().await else {
                    return Ok(None);
                };
                let message = message.map_err(|error| {
                    pan_error(
                        ErrorKind::BluezUnavailable,
                        "failed while receiving Bluetooth PAN link changes",
                        error,
                    )
                })?;
                if !panu_loss_trigger(&message, &peer)? {
                    continue;
                }
                if current_panu_attachment(&self.connection, &peer)
                    .await?
                    .is_none()
                {
                    self.finished = true;
                    return Ok(Some(PanuEvent::Lost(self.attachment.clone())));
                }
            }
        })
    }
}

async fn connect_panu(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<PanAttachment, CoreError> {
    if let Some(attachment) = current_panu_attachment(connection, peer).await? {
        return Ok(attachment);
    }

    let local_address = local_adapter_address_for_peer(connection, peer).await?;
    let previous_interfaces = kernel_interfaces_with_address(&local_address)?;
    let proxy = network_proxy(connection, peer).await?;
    let connect = async {
        proxy
            .call_method(CONNECT_METHOD, &(REMOTE_NAP_ROLE,))
            .await
            .map(Some)
    };
    let timeout = async {
        Timer::after(PANU_CONNECT_TIMEOUT).await;
        Ok::<Option<Message>, zbus::Error>(None)
    };

    match future::race(connect, timeout).await {
        Ok(Some(reply)) => {
            let interface: String = reply.body().deserialize().map_err(|error| {
                pan_error(
                    ErrorKind::ProtocolError,
                    "BlueZ returned an invalid PAN interface name",
                    error,
                )
            })?;
            // BlueZ returns the kernel-created name (normally bnepN), but udev may
            // immediately rename that netdev according to predictable-interface policy.
            // Give udev a short settle window, then resolve the live kernel name.
            Timer::after(PANU_INTERFACE_SETTLE_DELAY).await;
            let current_interfaces = kernel_interfaces_with_address(&local_address)?;
            let interface = select_panu_interface_name(
                &interface,
                &current_interfaces,
                Some(&previous_interfaces),
            )?;
            panu_attachment(peer, interface)
        }
        Ok(None) => match abort_pending_panu_connect(connection, peer).await {
            Ok(()) => Err(CoreError::new(
                ErrorKind::PanFailure,
                "Bluetooth PAN connection timed out and was cancelled",
            )),
            Err(cleanup) => Err(CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "Bluetooth PAN connection timed out and cleanup failed",
                cleanup
                    .diagnostic()
                    .unwrap_or_else(|| cleanup.message())
                    .to_owned(),
            )),
        },
        Err(error) => {
            if let Ok(Some(attachment)) = current_panu_attachment(connection, peer).await {
                return Ok(attachment);
            }
            if connect_is_in_progress(&error) {
                return Err(pan_error(
                    ErrorKind::InvalidState,
                    "another Bluetooth PAN connection attempt is already in progress",
                    error,
                ));
            }
            if connect_is_already_connected(&error) {
                return Err(CoreError::new(
                    ErrorKind::ProtocolError,
                    "BlueZ reported an existing PAN connection without an active interface",
                ));
            }
            Err(connect_error(error))
        }
    }
}

async fn abort_pending_panu_connect(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<(), CoreError> {
    let proxy = network_proxy(connection, peer).await?;
    match proxy.call_method(DISCONNECT_METHOD, &()).await {
        Ok(_) => Ok(()),
        Err(error) if disconnect_is_already_absent(&error) => Ok(()),
        Err(error) => Err(disconnect_error(error)),
    }
}

async fn disconnect_panu(connection: &Connection, peer: &PeerHandle) -> Result<(), CoreError> {
    // Desired-state semantics: if the authoritative Network1 state is already down,
    // disconnect is complete without issuing another method call.
    if current_panu_attachment(connection, peer).await?.is_none() {
        return Ok(());
    }

    let proxy = network_proxy(connection, peer).await?;
    match proxy.call_method(DISCONNECT_METHOD, &()).await {
        Ok(_) => Ok(()),
        Err(error) if disconnect_is_already_absent(&error) => Ok(()),
        Err(error) => {
            // BlueZ Network1 has used generic Failed for some teardown paths. Re-read
            // the desired state before deciding that a failed method call is fatal.
            match current_panu_attachment(connection, peer).await {
                Ok(None) => Ok(()),
                Ok(Some(_)) | Err(_) => Err(disconnect_error(error)),
            }
        }
    }
}

async fn current_panu_attachment(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<Option<PanAttachment>, CoreError> {
    let proxy = network_proxy(connection, peer).await?;
    let connected: bool = match proxy.get_property(CONNECTED_PROPERTY).await {
        Ok(connected) => connected,
        Err(error) if network_interface_is_absent(&error) => return Ok(None),
        Err(error) => return Err(property_error(CONNECTED_PROPERTY, error)),
    };
    if !connected {
        return Ok(None);
    }
    let interface: String = proxy
        .get_property(INTERFACE_PROPERTY)
        .await
        .map_err(|error| property_error(INTERFACE_PROPERTY, error))?;
    let local_address = local_adapter_address_for_peer(connection, peer).await?;
    let current_interfaces = kernel_interfaces_with_address(&local_address)?;
    let interface = select_panu_interface_name(&interface, &current_interfaces, None)?;
    panu_attachment(peer, interface).map(Some)
}

async fn local_adapter_address_for_peer(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<String, CoreError> {
    let adapter_path = peer_adapter_path(peer)?;
    let proxy = Proxy::new(connection, BLUEZ_SERVICE, adapter_path, ADAPTER_INTERFACE)
        .await
        .map_err(|error| {
            pan_error(
                ErrorKind::BluezUnavailable,
                "failed to access the Bluetooth adapter for PAN interface resolution",
                error,
            )
        })?;
    proxy
        .get_property(ADDRESS_PROPERTY)
        .await
        .map_err(|error| property_error(ADDRESS_PROPERTY, error))
}

fn peer_adapter_path(peer: &PeerHandle) -> Result<&str, CoreError> {
    peer.as_str()
        .rsplit_once("/dev_")
        .map(|(adapter, _)| adapter)
        .filter(|adapter| !adapter.is_empty())
        .ok_or_else(|| {
            CoreError::new(
                ErrorKind::ProtocolError,
                "Bluetooth peer handle does not identify its adapter",
            )
        })
}

fn kernel_interfaces_with_address(address: &str) -> Result<HashSet<String>, CoreError> {
    let entries = fs::read_dir(SYS_CLASS_NET).map_err(|error| {
        pan_error(
            ErrorKind::PanFailure,
            "failed to inspect Linux network interfaces for the Bluetooth PAN link",
            error,
        )
    })?;
    let mut interfaces = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            pan_error(
                ErrorKind::PanFailure,
                "failed to inspect a Linux network interface for the Bluetooth PAN link",
                error,
            )
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let interface_address = match fs::read_to_string(entry.path().join("address")) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(pan_error(
                    ErrorKind::PanFailure,
                    "failed to read a Linux network-interface address for PAN resolution",
                    error,
                ));
            }
        };
        if interface_address.trim().eq_ignore_ascii_case(address) {
            interfaces.insert(name);
        }
    }
    Ok(interfaces)
}

fn select_panu_interface_name(
    reported: &str,
    current: &HashSet<String>,
    previous: Option<&HashSet<String>>,
) -> Result<String, CoreError> {
    if current.contains(reported) {
        return Ok(reported.to_owned());
    }

    if let Some(previous) = previous {
        let new_interfaces = current
            .difference(previous)
            .cloned()
            .collect::<Vec<String>>();
        if new_interfaces.len() == 1 {
            return Ok(new_interfaces[0].clone());
        }
    } else if current.len() == 1 {
        return Ok(current.iter().next().expect("one interface exists").clone());
    }

    let mut candidates = current.iter().cloned().collect::<Vec<_>>();
    candidates.sort();
    Err(CoreError::with_diagnostic(
        ErrorKind::PanFailure,
        "Bluetooth PAN interface was renamed but its current Linux name could not be resolved",
        format!("BlueZ reported {reported:?}; matching kernel interfaces: {candidates:?}"),
    ))
}

async fn network_proxy<'a>(
    connection: &'a Connection,
    peer: &'a PeerHandle,
) -> Result<Proxy<'a>, CoreError> {
    Proxy::new(connection, BLUEZ_SERVICE, peer.as_str(), NETWORK_INTERFACE)
        .await
        .map_err(|error| {
            pan_error(
                ErrorKind::PanFailure,
                "failed to access the Bluetooth PAN network profile",
                error,
            )
        })
}

fn panu_attachment(peer: &PeerHandle, interface: String) -> Result<PanAttachment, CoreError> {
    if interface.trim().is_empty() {
        return Err(CoreError::new(
            ErrorKind::ProtocolError,
            "BlueZ returned an empty PAN interface name",
        ));
    }
    Ok(PanAttachment {
        role: PanRole::Panu,
        interface: NetworkInterfaceHandle::new(interface)?,
        peer: Some(peer.clone()),
    })
}

fn validate_panu_attachment(attachment: &PanAttachment) -> Result<(), CoreError> {
    if attachment.role != PanRole::Panu {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "PANU event subscription requires a PANU attachment",
        ));
    }
    if attachment.peer.is_none() {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "PANU event subscription requires a peer",
        ));
    }
    Ok(())
}

async fn bluez_signal_stream(connection: &Connection) -> Result<MessageStream, CoreError> {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BLUEZ_SERVICE)
        .map_err(|error| {
            pan_error(
                ErrorKind::BluezUnavailable,
                "failed to build the Bluetooth PAN signal subscription",
                error,
            )
        })?
        .build();
    MessageStream::for_match_rule(rule, connection, Some(SIGNAL_QUEUE_CAPACITY))
        .await
        .map_err(|error| {
            pan_error(
                ErrorKind::BluezUnavailable,
                "failed to subscribe to Bluetooth PAN changes",
                error,
            )
        })
}

fn panu_loss_trigger(message: &Message, peer: &PeerHandle) -> Result<bool, CoreError> {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str());
    let member = header.member().map(|name| name.as_str());

    if interface == Some(OBJECT_MANAGER_INTERFACE) && member == Some(INTERFACES_REMOVED) {
        let (path, interfaces): (OwnedObjectPath, Vec<OwnedInterfaceName>) =
            message.body().deserialize().map_err(|error| {
                pan_error(
                    ErrorKind::ProtocolError,
                    "failed to decode a Bluetooth PAN InterfacesRemoved signal",
                    error,
                )
            })?;
        return Ok(path.as_str() == peer.as_str()
            && interfaces
                .iter()
                .any(|name| matches!(name.as_str(), NETWORK_INTERFACE | DEVICE_INTERFACE)));
    }

    if interface != Some(PROPERTIES_INTERFACE)
        || member != Some(PROPERTIES_CHANGED)
        || header.path().map(|path| path.as_str()) != Some(peer.as_str())
    {
        return Ok(false);
    }

    let (interface_name, changed, invalidated): (
        OwnedInterfaceName,
        HashMap<String, OwnedValue>,
        Vec<String>,
    ) = message.body().deserialize().map_err(|error| {
        pan_error(
            ErrorKind::ProtocolError,
            "failed to decode a Bluetooth PAN PropertiesChanged signal",
            error,
        )
    })?;

    Ok(properties_change_can_affect_panu_state(
        interface_name.as_str(),
        &changed,
        &invalidated,
    ))
}

fn properties_change_can_affect_panu_state(
    interface: &str,
    changed: &HashMap<String, OwnedValue>,
    invalidated: &[String],
) -> bool {
    let properties = match interface {
        NETWORK_INTERFACE => Some(&[CONNECTED_PROPERTY, INTERFACE_PROPERTY][..]),
        DEVICE_INTERFACE => Some(&[CONNECTED_PROPERTY][..]),
        _ => None,
    };
    let Some(properties) = properties else {
        return false;
    };
    properties.iter().any(|property| {
        changed.contains_key(*property)
            || invalidated
                .iter()
                .any(|invalid| invalid.as_str() == *property)
    })
}

fn connect_is_already_connected(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.bluez.Error.AlreadyConnected"
    )
}

fn connect_is_in_progress(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::MethodError(name, detail, _) => {
            name.as_str() == "org.bluez.Error.InProgress"
                || (name.as_str() == "org.bluez.Error.Failed"
                    && detail
                        .as_deref()
                        .is_some_and(|message| message.contains("already in progress")))
        }
        _ => false,
    }
}

fn connect_error(error: zbus::Error) -> CoreError {
    let (kind, message) = match &error {
        zbus::Error::MethodError(name, _, _) => connect_method_error(name.as_str()),
        _ => (
            ErrorKind::BluezUnavailable,
            "Bluetooth PAN connection failed because BlueZ is unavailable",
        ),
    };
    pan_error(kind, message, error)
}

fn connect_method_error(name: &str) -> (ErrorKind, &'static str) {
    match name {
        "org.bluez.Error.ConnectionAttemptFailed" => (
            ErrorKind::PanFailure,
            "Bluetooth PAN connection attempt failed",
        ),
        "org.bluez.Error.NotAuthorized" | "org.freedesktop.DBus.Error.AccessDenied" => (
            ErrorKind::AuthenticationFailed,
            "Bluetooth PAN connection was not authorized",
        ),
        "org.bluez.Error.NotReady" => (
            ErrorKind::PanFailure,
            "Bluetooth peer is not ready for a PAN connection",
        ),
        "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject" => (
            ErrorKind::InvalidState,
            "Bluetooth peer is no longer available",
        ),
        "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.bluez.Error.NotSupported"
        | "org.bluez.Error.NotAvailable" => (
            ErrorKind::CapabilityUnavailable,
            "Bluetooth peer does not expose a usable PAN network profile",
        ),
        _ => (ErrorKind::PanFailure, "Bluetooth PAN connection failed"),
    }
}

fn disconnect_error(error: zbus::Error) -> CoreError {
    let kind = match &error {
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.UnknownMethod"
                    | "org.freedesktop.DBus.Error.UnknownInterface"
                    | "org.bluez.Error.NotSupported"
                    | "org.bluez.Error.NotAvailable"
            ) =>
        {
            ErrorKind::CapabilityUnavailable
        }
        zbus::Error::MethodError(_, _, _) => ErrorKind::PanFailure,
        _ => ErrorKind::BluezUnavailable,
    };
    pan_error(kind, "failed to disconnect the Bluetooth PAN link", error)
}

fn disconnect_is_already_absent(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.NotConnected"
                    | "org.bluez.Error.DoesNotExist"
                    | "org.freedesktop.DBus.Error.UnknownObject"
            )
    )
}

fn network_interface_is_absent(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.DoesNotExist"
                    | "org.freedesktop.DBus.Error.UnknownObject"
                    | "org.freedesktop.DBus.Error.UnknownInterface"
            )
    )
}

fn property_error(property: &'static str, error: zbus::Error) -> CoreError {
    pan_error(
        ErrorKind::ProtocolError,
        format!("failed to read Bluetooth PAN {property} state"),
        error,
    )
}

fn pan_error(
    kind: ErrorKind,
    message: impl Into<String>,
    error: impl std::fmt::Display,
) -> CoreError {
    CoreError::with_diagnostic(kind, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> PeerHandle {
        PeerHandle::new("/org/bluez/hci0/dev_00_11_22_33_44_55").unwrap()
    }

    #[test]
    fn panu_attachment_preserves_peer_and_interface() {
        let attachment = panu_attachment(&peer(), "bnep7".to_owned()).unwrap();
        assert_eq!(attachment.role, PanRole::Panu);
        assert_eq!(attachment.interface.as_str(), "bnep7");
        assert_eq!(attachment.peer, Some(peer()));
    }

    #[test]
    fn empty_interface_is_protocol_error() {
        assert_eq!(
            panu_attachment(&peer(), "   ".to_owned())
                .unwrap_err()
                .kind(),
            ErrorKind::ProtocolError
        );
    }

    #[test]
    fn panu_subscription_rejects_non_panu_attachment() {
        let attachment = PanAttachment {
            role: PanRole::Nap,
            interface: NetworkInterfaceHandle::new("bnep0").unwrap(),
            peer: None,
        };
        assert_eq!(
            validate_panu_attachment(&attachment).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn network_and_device_connected_changes_trigger_state_refresh() {
        let mut changed = HashMap::new();
        changed.insert(CONNECTED_PROPERTY.to_owned(), OwnedValue::from(false));
        assert!(properties_change_can_affect_panu_state(
            NETWORK_INTERFACE,
            &changed,
            &[]
        ));
        assert!(properties_change_can_affect_panu_state(
            DEVICE_INTERFACE,
            &changed,
            &[]
        ));
    }

    #[test]
    fn unrelated_properties_do_not_trigger_state_refresh() {
        let mut changed = HashMap::new();
        changed.insert("Alias".to_owned(), OwnedValue::from(false));
        assert!(!properties_change_can_affect_panu_state(
            DEVICE_INTERFACE,
            &changed,
            &[]
        ));
    }

    #[test]
    fn connection_attempt_failure_maps_to_pan_failure() {
        assert_eq!(
            connect_method_error("org.bluez.Error.ConnectionAttemptFailed"),
            (
                ErrorKind::PanFailure,
                "Bluetooth PAN connection attempt failed"
            )
        );
    }

    #[test]
    fn unavailable_network_profile_maps_to_capability_error() {
        assert_eq!(
            connect_method_error("org.freedesktop.DBus.Error.UnknownMethod").0,
            ErrorKind::CapabilityUnavailable
        );
    }

    #[test]
    fn reported_kernel_interface_is_preferred_when_it_still_exists() {
        let current = HashSet::from(["bnep0".to_owned(), "enx001122334455".to_owned()]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, None).unwrap(),
            "bnep0"
        );
    }

    #[test]
    fn newly_renamed_interface_is_selected_after_connect() {
        let previous = HashSet::from(["enxaaaaaaaaaaaa".to_owned()]);
        let current = HashSet::from(["enxaaaaaaaaaaaa".to_owned(), "enx001122334455".to_owned()]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, Some(&previous)).unwrap(),
            "enx001122334455"
        );
    }

    #[test]
    fn existing_connection_resolves_unique_renamed_interface() {
        let current = HashSet::from(["enx001122334455".to_owned()]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, None).unwrap(),
            "enx001122334455"
        );
    }

    #[test]
    fn ambiguous_renamed_interfaces_fail_instead_of_guessing() {
        let current = HashSet::from(["enx001122334455".to_owned(), "enx001122334456".to_owned()]);
        assert_eq!(
            select_panu_interface_name("bnep0", &current, None)
                .unwrap_err()
                .kind(),
            ErrorKind::PanFailure
        );
    }
}

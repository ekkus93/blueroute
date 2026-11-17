//! Pairing through BlueZ: the single-operation pairing control, the no-input/no-output pairing
//! agent, incoming pairing windows, outgoing pairing and trust.

use super::{
    ADAPTER_INTERFACE, AGENT_MANAGER_INTERFACE, BLUEZ_AGENT_MANAGER_PATH, BLUEZ_SERVICE,
    CANCEL_PAIRING_METHOD, DEVICE_INTERFACE, DISCOVERABLE_PROPERTY, PAIR_METHOD, PAIRABLE_PROPERTY,
    PAIRING_AGENT_CAPABILITY, PAIRING_AGENT_PATH, PAIRING_TIMEOUT, REGISTER_AGENT_METHOD,
    REQUEST_DEFAULT_AGENT_METHOD, TRUSTED_PROPERTY, UNREGISTER_AGENT_METHOD, ensure_adapter_exists,
    is_device_object_path_for_adapter_path, managed_objects, peer_from_properties,
};
use crate::{AdapterHandle, DiscoveredPeer, IncomingPairingWindow, PeerHandle};
use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind};
use futures_lite::future::race;
use std::sync::{Arc, Mutex};
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, Proxy};

#[derive(Debug, Default)]
pub(super) struct PairingControl {
    state: Mutex<PairingControlState>,
}

#[derive(Debug, Default)]
struct PairingControlState {
    active_peer: Option<String>,
    incoming_adapter: Option<String>,
}

impl PairingControl {
    fn begin(self: &Arc<Self>, peer: &PeerHandle) -> Result<PairingPermit, CoreError> {
        let mut state = self.lock_state()?;
        if state.active_peer.is_some() || state.incoming_adapter.is_some() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "another Bluetooth pairing operation is already active",
            ));
        }
        let peer = peer.as_str().to_owned();
        state.active_peer = Some(peer.clone());
        Ok(PairingPermit {
            control: Arc::clone(self),
            peer,
        })
    }

    fn begin_incoming(&self, adapter: &AdapterHandle) -> Result<(), CoreError> {
        let mut state = self.lock_state()?;
        if state.active_peer.is_some() || state.incoming_adapter.is_some() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "another Bluetooth pairing operation is already active",
            ));
        }
        state.incoming_adapter = Some(adapter.as_str().to_owned());
        Ok(())
    }

    fn end_incoming(&self, adapter: &AdapterHandle) -> Result<(), CoreError> {
        let mut state = self.lock_state()?;
        if state.incoming_adapter.as_deref() != Some(adapter.as_str()) {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "incoming Bluetooth pairing window is not active for this adapter",
            ));
        }
        state.incoming_adapter = None;
        Ok(())
    }

    fn clear_incoming(&self, adapter: &AdapterHandle) {
        if let Ok(mut state) = self.state.lock()
            && state.incoming_adapter.as_deref() == Some(adapter.as_str())
        {
            state.incoming_adapter = None;
        }
    }

    fn authorizes(&self, device: &OwnedObjectPath) -> bool {
        self.state
            .lock()
            .map(|state| {
                state.active_peer.as_deref() == Some(device.as_str())
                    || state.incoming_adapter.as_deref().is_some_and(|adapter| {
                        is_device_object_path_for_adapter_path(device.as_str(), adapter)
                    })
            })
            .unwrap_or(false)
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, PairingControlState>, CoreError> {
        self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::Internal,
                "Bluetooth pairing authorization state is unavailable",
            )
        })
    }
}

#[derive(Debug)]
struct PairingPermit {
    control: Arc<PairingControl>,
    peer: String,
}

impl Drop for PairingPermit {
    fn drop(&mut self) {
        if let Ok(mut state) = self.control.state.lock()
            && state.active_peer.as_deref() == Some(self.peer.as_str())
        {
            state.active_peer = None;
        }
    }
}

#[derive(Clone, Debug)]
struct PairingAgent {
    control: Arc<PairingControl>,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error", impl_display = true)]
enum PairingAgentError {
    Rejected(String),
}

impl PairingAgent {
    fn require_authorized(&self, device: &OwnedObjectPath) -> Result<(), PairingAgentError> {
        if self.control.authorizes(device) {
            Ok(())
        } else {
            Err(PairingAgentError::Rejected(
                "BlueRoute did not authorize this pairing request".to_owned(),
            ))
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl PairingAgent {
    fn release(&self) {}

    fn request_pin_code(&self, device: OwnedObjectPath) -> Result<String, PairingAgentError> {
        self.require_authorized(&device)?;
        Err(PairingAgentError::Rejected(
            "BlueRoute NoInputNoOutput pairing cannot provide a PIN code".to_owned(),
        ))
    }

    fn display_pin_code(&self, _device: OwnedObjectPath, _pin_code: String) {}

    fn request_passkey(&self, device: OwnedObjectPath) -> Result<u32, PairingAgentError> {
        self.require_authorized(&device)?;
        Err(PairingAgentError::Rejected(
            "BlueRoute NoInputNoOutput pairing cannot provide a passkey".to_owned(),
        ))
    }

    fn display_passkey(&self, _device: OwnedObjectPath, _passkey: u32, _entered: u16) {}

    fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        _passkey: u32,
    ) -> Result<(), PairingAgentError> {
        self.require_authorized(&device)?;
        Err(PairingAgentError::Rejected(
            "BlueRoute NoInputNoOutput pairing cannot confirm a displayed passkey".to_owned(),
        ))
    }

    fn request_authorization(&self, device: OwnedObjectPath) -> Result<(), PairingAgentError> {
        self.require_authorized(&device)
    }

    fn authorize_service(
        &self,
        device: OwnedObjectPath,
        _uuid: String,
    ) -> Result<(), PairingAgentError> {
        self.require_authorized(&device)
    }

    fn cancel(&self) {}
}

enum PairCompletion {
    Paired,
    TimedOut,
}

async fn ensure_pairing_agent(
    connection: &Connection,
    control: &Arc<PairingControl>,
) -> Result<(), CoreError> {
    connection
        .object_server()
        .at(
            PAIRING_AGENT_PATH,
            PairingAgent {
                control: Arc::clone(control),
            },
        )
        .await
        .map_err(|error| {
            pairing_agent_registration_error("serve the BlueRoute pairing agent", error)
        })?;

    let manager = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        BLUEZ_AGENT_MANAGER_PATH,
        AGENT_MANAGER_INTERFACE,
    )
    .await
    .map_err(|error| {
        pairing_agent_registration_error("create the BlueZ agent-manager proxy", error)
    })?;
    let agent_path = OwnedObjectPath::try_from(PAIRING_AGENT_PATH).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::Internal,
            "BlueRoute pairing-agent object path is invalid",
            error.to_string(),
        )
    })?;

    match manager
        .call_method(
            REGISTER_AGENT_METHOD,
            &(agent_path, PAIRING_AGENT_CAPABILITY),
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.bluez.Error.AlreadyExists" =>
        {
            Ok(())
        }
        Err(error) => Err(pairing_agent_registration_error(
            "register the BlueRoute pairing agent",
            error,
        )),
    }
}

fn pairing_agent_registration_error(
    operation: &'static str,
    error: impl std::fmt::Display,
) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::CapabilityUnavailable,
        format!("failed to {operation}"),
        error.to_string(),
    )
}

async fn request_default_pairing_agent(connection: &Connection) -> Result<(), CoreError> {
    let manager = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        BLUEZ_AGENT_MANAGER_PATH,
        AGENT_MANAGER_INTERFACE,
    )
    .await
    .map_err(|error| {
        pairing_agent_registration_error("create the BlueZ agent-manager proxy", error)
    })?;
    let agent_path = OwnedObjectPath::try_from(PAIRING_AGENT_PATH).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::Internal,
            "BlueRoute pairing-agent object path is invalid",
            error.to_string(),
        )
    })?;
    manager
        .call_method(REQUEST_DEFAULT_AGENT_METHOD, &(agent_path,))
        .await
        .map(|_| ())
        .map_err(|error| {
            pairing_agent_registration_error("make the BlueRoute pairing agent the default", error)
        })
}

async fn unregister_pairing_agent(connection: &Connection) -> Result<(), CoreError> {
    let manager = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        BLUEZ_AGENT_MANAGER_PATH,
        AGENT_MANAGER_INTERFACE,
    )
    .await
    .map_err(|error| {
        pairing_agent_registration_error("create the BlueZ agent-manager proxy", error)
    })?;
    let agent_path = OwnedObjectPath::try_from(PAIRING_AGENT_PATH).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::Internal,
            "BlueRoute pairing-agent object path is invalid",
            error.to_string(),
        )
    })?;
    match manager
        .call_method(UNREGISTER_AGENT_METHOD, &(agent_path,))
        .await
    {
        Ok(_) => Ok(()),
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.bluez.Error.DoesNotExist" =>
        {
            Ok(())
        }
        Err(error) => Err(pairing_agent_registration_error(
            "unregister the BlueRoute pairing agent",
            error,
        )),
    }
}

pub(super) async fn begin_incoming_pairing(
    connection: &Connection,
    control: &Arc<PairingControl>,
    adapter: &AdapterHandle,
) -> Result<IncomingPairingWindow, CoreError> {
    let current = ensure_adapter_exists(connection, adapter).await?;
    if !current.powered {
        return Err(CoreError::new(
            ErrorKind::AdapterDisabled,
            "Bluetooth adapter must be powered before incoming pairing can start",
        ));
    }

    let proxy = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        adapter.as_str(),
        ADAPTER_INTERFACE,
    )
    .await
    .map_err(|error| incoming_pairing_error("create the Bluetooth adapter proxy", error))?;
    let restore_pairable: bool = proxy
        .get_property(PAIRABLE_PROPERTY)
        .await
        .map_err(|error| incoming_pairing_error("read Bluetooth Pairable state", error))?;
    let restore_discoverable: bool = proxy
        .get_property(DISCOVERABLE_PROPERTY)
        .await
        .map_err(|error| incoming_pairing_error("read Bluetooth Discoverable state", error))?;

    ensure_pairing_agent(connection, control).await?;
    control.begin_incoming(adapter)?;
    if let Err(error) = request_default_pairing_agent(connection).await {
        control.clear_incoming(adapter);
        return Err(error);
    }
    if let Err(error) = proxy.set_property(PAIRABLE_PROPERTY, true).await {
        control.clear_incoming(adapter);
        let _ = unregister_pairing_agent(connection).await;
        return Err(incoming_pairing_property_error(
            "enable Bluetooth Pairable state",
            error,
        ));
    }
    if let Err(error) = proxy.set_property(DISCOVERABLE_PROPERTY, true).await {
        let _ = proxy
            .set_property(PAIRABLE_PROPERTY, restore_pairable)
            .await;
        control.clear_incoming(adapter);
        let _ = unregister_pairing_agent(connection).await;
        return Err(incoming_pairing_property_error(
            "enable Bluetooth Discoverable state",
            error,
        ));
    }

    Ok(IncomingPairingWindow {
        adapter: adapter.clone(),
        restore_discoverable,
        restore_pairable,
    })
}

pub(super) async fn end_incoming_pairing(
    connection: &Connection,
    control: &Arc<PairingControl>,
    window: IncomingPairingWindow,
) -> Result<(), CoreError> {
    control.end_incoming(&window.adapter)?;

    let mut first_error = None;
    match Proxy::new(
        connection,
        BLUEZ_SERVICE,
        window.adapter.as_str(),
        ADAPTER_INTERFACE,
    )
    .await
    {
        Ok(proxy) => {
            if let Err(error) = proxy
                .set_property(DISCOVERABLE_PROPERTY, window.restore_discoverable)
                .await
            {
                first_error = Some(incoming_pairing_property_error(
                    "restore Bluetooth Discoverable state",
                    error,
                ));
            }
            if let Err(error) = proxy
                .set_property(PAIRABLE_PROPERTY, window.restore_pairable)
                .await
                && first_error.is_none()
            {
                first_error = Some(incoming_pairing_property_error(
                    "restore Bluetooth Pairable state",
                    error,
                ));
            }
        }
        Err(error) => {
            first_error = Some(incoming_pairing_error(
                "create the Bluetooth adapter proxy during pairing cleanup",
                error,
            ));
        }
    }

    if let Err(error) = unregister_pairing_agent(connection).await
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn incoming_pairing_property_error(operation: &'static str, error: zbus::fdo::Error) -> CoreError {
    let kind = match &error {
        zbus::fdo::Error::UnknownObject(_) => ErrorKind::MissingAdapter,
        zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_) => {
            ErrorKind::AuthenticationFailed
        }
        _ => ErrorKind::CapabilityUnavailable,
    };
    CoreError::with_diagnostic(kind, format!("failed to {operation}"), error.to_string())
}

fn incoming_pairing_error(operation: &'static str, error: zbus::Error) -> CoreError {
    let kind = match &error {
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject"
            ) =>
        {
            ErrorKind::MissingAdapter
        }
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.NotAuthorized" | "org.freedesktop.DBus.Error.AccessDenied"
            ) =>
        {
            ErrorKind::AuthenticationFailed
        }
        _ => ErrorKind::CapabilityUnavailable,
    };
    CoreError::with_diagnostic(kind, format!("failed to {operation}"), error.to_string())
}

async fn peer_by_handle(
    connection: &Connection,
    peer: &PeerHandle,
) -> Result<DiscoveredPeer, CoreError> {
    let objects = managed_objects(connection).await?;
    let Some((path, interfaces)) = objects
        .iter()
        .find(|(path, _)| path.as_str() == peer.as_str())
    else {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "Bluetooth peer is no longer available",
        ));
    };
    let Some(properties) = interfaces
        .iter()
        .find_map(|(name, properties)| (name.as_str() == DEVICE_INTERFACE).then_some(properties))
    else {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "Bluetooth peer no longer exposes BlueZ Device1",
        ));
    };
    peer_from_properties(path, properties)
}

pub(super) async fn pair_peer(
    connection: &Connection,
    control: &Arc<PairingControl>,
    peer: &PeerHandle,
) -> Result<(), CoreError> {
    let current = peer_by_handle(connection, peer).await?;
    if current.paired {
        return Ok(());
    }

    ensure_pairing_agent(connection, control).await?;
    let _permit = control.begin(peer)?;
    let proxy = Proxy::new(connection, BLUEZ_SERVICE, peer.as_str(), DEVICE_INTERFACE)
        .await
        .map_err(pairing_error)?;

    let completion = race(
        async {
            proxy
                .call_method(PAIR_METHOD, &())
                .await
                .map(|_| PairCompletion::Paired)
        },
        async {
            Timer::after(PAIRING_TIMEOUT).await;
            Ok::<PairCompletion, zbus::Error>(PairCompletion::TimedOut)
        },
    )
    .await;

    match completion {
        Ok(PairCompletion::Paired) => Ok(()),
        Ok(PairCompletion::TimedOut) => {
            let _ = proxy.call_method(CANCEL_PAIRING_METHOD, &()).await;
            Err(pairing_timeout_error())
        }
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.bluez.Error.AlreadyExists" =>
        {
            Ok(())
        }
        Err(error) => Err(pairing_error(error)),
    }
}

fn pairing_timeout_error() -> CoreError {
    CoreError::new(ErrorKind::PairingFailed, "Bluetooth pairing timed out")
}

fn pairing_error(error: zbus::Error) -> CoreError {
    let (kind, message) = match &error {
        zbus::Error::MethodError(name, _, _) => pairing_method_error(name.as_str()),
        zbus::Error::FDO(error)
            if matches!(
                error.as_ref(),
                zbus::fdo::Error::NoReply(_)
                    | zbus::fdo::Error::TimedOut(_)
                    | zbus::fdo::Error::Timeout(_)
            ) =>
        {
            (ErrorKind::PairingFailed, "Bluetooth pairing timed out")
        }
        _ => (ErrorKind::PairingFailed, "Bluetooth pairing failed"),
    };
    CoreError::with_diagnostic(kind, message, error.to_string())
}

fn pairing_method_error(name: &str) -> (ErrorKind, &'static str) {
    match name {
        "org.bluez.Error.AuthenticationRejected" => (
            ErrorKind::AuthenticationFailed,
            "Bluetooth pairing was rejected",
        ),
        "org.bluez.Error.AuthenticationCanceled" => (
            ErrorKind::AuthenticationFailed,
            "Bluetooth pairing was canceled",
        ),
        "org.bluez.Error.AuthenticationFailed" => (
            ErrorKind::AuthenticationFailed,
            "Bluetooth authentication failed",
        ),
        "org.bluez.Error.AuthenticationTimeout" => {
            (ErrorKind::PairingFailed, "Bluetooth pairing timed out")
        }
        "org.bluez.Error.ConnectionAttemptFailed" => (
            ErrorKind::PairingFailed,
            "Bluetooth connection attempt failed",
        ),
        "org.bluez.Error.InvalidArguments" => (
            ErrorKind::InvalidInput,
            "Bluetooth pairing request is invalid",
        ),
        "org.bluez.Error.InProgress" => (
            ErrorKind::InvalidState,
            "Bluetooth pairing is already in progress",
        ),
        "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject" => (
            ErrorKind::InvalidState,
            "Bluetooth peer is no longer available",
        ),
        _ => (ErrorKind::PairingFailed, "Bluetooth pairing failed"),
    }
}

pub(super) async fn set_peer_trusted(
    connection: &Connection,
    peer: &PeerHandle,
    trusted: bool,
) -> Result<(), CoreError> {
    let current = peer_by_handle(connection, peer).await?;
    if trusted && !current.paired {
        return Err(CoreError::new(
            ErrorKind::AuthenticationFailed,
            "pair the Bluetooth peer before trusting it",
        ));
    }
    if current.trusted == trusted {
        return Ok(());
    }

    let proxy = Proxy::new(connection, BLUEZ_SERVICE, peer.as_str(), DEVICE_INTERFACE)
        .await
        .map_err(trust_error)?;
    proxy
        .set_property(TRUSTED_PROPERTY, trusted)
        .await
        .map_err(trust_property_error)
}

fn trust_property_error(error: zbus::fdo::Error) -> CoreError {
    let kind = match &error {
        zbus::fdo::Error::UnknownObject(_) => ErrorKind::InvalidState,
        zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_) => {
            ErrorKind::AuthenticationFailed
        }
        _ => ErrorKind::AuthenticationFailed,
    };
    CoreError::with_diagnostic(
        kind,
        "failed to change Bluetooth trust state",
        error.to_string(),
    )
}

fn trust_error(error: zbus::Error) -> CoreError {
    let kind = match &error {
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject"
            ) =>
        {
            ErrorKind::InvalidState
        }
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.NotAuthorized" | "org.freedesktop.DBus.Error.AccessDenied"
            ) =>
        {
            ErrorKind::AuthenticationFailed
        }
        _ => ErrorKind::AuthenticationFailed,
    };
    CoreError::with_diagnostic(
        kind,
        "failed to change Bluetooth trust state",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn pairing_agent_only_authorizes_the_active_peer() {
        let control = Arc::new(PairingControl::default());
        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let other = OwnedObjectPath::try_from("/org/bluez/hci0/dev_11_22_33_44_55_66").unwrap();
        let active = OwnedObjectPath::try_from(peer.as_str()).unwrap();
        let permit = control.begin(&peer).unwrap();
        assert!(control.authorizes(&active));
        assert!(!control.authorizes(&other));
        drop(permit);
        assert!(!control.authorizes(&active));
    }

    #[test]
    fn pairing_control_rejects_concurrent_pairing_operations() {
        let control = Arc::new(PairingControl::default());
        let first = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let second = PeerHandle::new("/org/bluez/hci0/dev_11_22_33_44_55_66").unwrap();
        let _permit = control.begin(&first).unwrap();
        let error = match control.begin(&second) {
            Ok(_) => panic!("concurrent pairing unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ErrorKind::InvalidState);
    }

    #[test]
    fn pairing_rejection_and_timeout_have_distinct_typed_errors() {
        assert_eq!(
            pairing_method_error("org.bluez.Error.AuthenticationRejected"),
            (
                ErrorKind::AuthenticationFailed,
                "Bluetooth pairing was rejected"
            )
        );
        assert_eq!(
            pairing_method_error("org.bluez.Error.AuthenticationTimeout"),
            (ErrorKind::PairingFailed, "Bluetooth pairing timed out")
        );
        assert_eq!(pairing_timeout_error().kind(), ErrorKind::PairingFailed);
    }

    #[test]
    fn pairing_input_requests_are_rejected_for_no_input_no_output_agent() {
        let control = Arc::new(PairingControl::default());
        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let _permit = control.begin(&peer).unwrap();
        let agent = PairingAgent { control };
        let path = OwnedObjectPath::try_from(peer.as_str()).unwrap();
        assert!(agent.request_pin_code(path.clone()).is_err());
        assert!(agent.request_passkey(path.clone()).is_err());
        assert!(agent.request_confirmation(path, 123456).is_err());
    }

    #[test]
    fn incoming_pairing_window_authorizes_only_selected_adapter_devices() {
        let control = Arc::new(PairingControl::default());
        let adapter = AdapterHandle::new("/org/bluez/hci0").unwrap();
        control.begin_incoming(&adapter).unwrap();
        let allowed = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let other = OwnedObjectPath::try_from("/org/bluez/hci1/dev_AA_BB_CC_DD_EE_FF").unwrap();
        assert!(control.authorizes(&allowed));
        assert!(!control.authorizes(&other));
        control.end_incoming(&adapter).unwrap();
        assert!(!control.authorizes(&allowed));
    }

    #[test]
    fn incoming_and_outgoing_pairing_modes_are_mutually_exclusive() {
        let control = Arc::new(PairingControl::default());
        let adapter = AdapterHandle::new("/org/bluez/hci0").unwrap();
        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        control.begin_incoming(&adapter).unwrap();
        assert_eq!(
            control.begin(&peer).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        control.end_incoming(&adapter).unwrap();
        let _permit = control.begin(&peer).unwrap();
        assert_eq!(
            control.begin_incoming(&adapter).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
    }

    const DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
    const OTHER_DEVICE: &str = "/org/bluez/hci0/dev_11_22_33_44_55_66";
    const OTHER_ADAPTER_DEVICE: &str = "/org/bluez/hci1/dev_AA_BB_CC_DD_EE_FF";
    const NESTED_PATH: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/sep1";
    const UNAUTHORIZED: &str = "BlueRoute did not authorize this pairing request";

    fn method_error(name: &str) -> zbus::Error {
        let message = zbus::message::Message::method_call(DEVICE, "Pair")
            .unwrap()
            .build(&())
            .unwrap();
        zbus::Error::MethodError(
            zbus::names::OwnedErrorName::try_from(name).unwrap(),
            Some("detail".to_owned()),
            message,
        )
    }

    fn fdo(error: zbus::fdo::Error) -> zbus::Error {
        zbus::Error::FDO(Box::new(error))
    }

    fn assert_error(error: CoreError, kind: ErrorKind, message: &str, case: &str) {
        assert_eq!(error.kind(), kind, "{case}");
        assert_eq!(error.message(), message, "{case}");
        assert!(error.diagnostic().is_some(), "{case}: diagnostic missing");
    }

    #[test]
    fn every_pairing_method_error_name_maps_to_its_kind_and_message() {
        use ErrorKind::*;
        let cases = [
            (
                "org.bluez.Error.AuthenticationRejected",
                AuthenticationFailed,
                "Bluetooth pairing was rejected",
            ),
            (
                "org.bluez.Error.AuthenticationCanceled",
                AuthenticationFailed,
                "Bluetooth pairing was canceled",
            ),
            (
                "org.bluez.Error.AuthenticationFailed",
                AuthenticationFailed,
                "Bluetooth authentication failed",
            ),
            (
                "org.bluez.Error.AuthenticationTimeout",
                PairingFailed,
                "Bluetooth pairing timed out",
            ),
            (
                "org.bluez.Error.ConnectionAttemptFailed",
                PairingFailed,
                "Bluetooth connection attempt failed",
            ),
            (
                "org.bluez.Error.InvalidArguments",
                InvalidInput,
                "Bluetooth pairing request is invalid",
            ),
            (
                "org.bluez.Error.InProgress",
                InvalidState,
                "Bluetooth pairing is already in progress",
            ),
            (
                "org.bluez.Error.DoesNotExist",
                InvalidState,
                "Bluetooth peer is no longer available",
            ),
            (
                "org.freedesktop.DBus.Error.UnknownObject",
                InvalidState,
                "Bluetooth peer is no longer available",
            ),
            (
                "org.bluez.Error.AlreadyExists",
                PairingFailed,
                "Bluetooth pairing failed",
            ),
            (
                "org.bluez.Error.Failed",
                PairingFailed,
                "Bluetooth pairing failed",
            ),
        ];
        for (name, kind, message) in cases {
            assert_eq!(pairing_method_error(name), (kind, message), "{name}");
            assert_error(pairing_error(method_error(name)), kind, message, name);
        }
    }

    #[test]
    fn pairing_transport_errors_map_timeouts_apart_from_other_failures() {
        let timed_out = "Bluetooth pairing timed out";
        let failed = "Bluetooth pairing failed";
        let cases = [
            (
                "no reply",
                fdo(zbus::fdo::Error::NoReply("x".to_owned())),
                timed_out,
            ),
            (
                "timed out",
                fdo(zbus::fdo::Error::TimedOut("x".to_owned())),
                timed_out,
            ),
            (
                "timeout",
                fdo(zbus::fdo::Error::Timeout("x".to_owned())),
                timed_out,
            ),
            (
                "fdo failed",
                fdo(zbus::fdo::Error::Failed("x".to_owned())),
                failed,
            ),
            (
                "transport failure",
                zbus::Error::Failure("x".to_owned()),
                failed,
            ),
        ];
        for (case, error, message) in cases {
            assert_error(
                pairing_error(error),
                ErrorKind::PairingFailed,
                message,
                case,
            );
        }
    }

    #[test]
    fn trust_errors_map_missing_peers_apart_from_refusals() {
        use ErrorKind::*;
        let message = "failed to change Bluetooth trust state";
        let cases = [
            ("org.bluez.Error.DoesNotExist", InvalidState),
            ("org.freedesktop.DBus.Error.UnknownObject", InvalidState),
            ("org.bluez.Error.NotAuthorized", AuthenticationFailed),
            (
                "org.freedesktop.DBus.Error.AccessDenied",
                AuthenticationFailed,
            ),
            ("org.bluez.Error.Failed", AuthenticationFailed),
        ];
        for (name, kind) in cases {
            assert_error(trust_error(method_error(name)), kind, message, name);
        }
        assert_error(
            trust_error(zbus::Error::Failure("x".to_owned())),
            AuthenticationFailed,
            message,
            "transport failure",
        );

        let property_cases = [
            (
                "unknown object",
                zbus::fdo::Error::UnknownObject("x".to_owned()),
                InvalidState,
            ),
            (
                "access denied",
                zbus::fdo::Error::AccessDenied("x".to_owned()),
                AuthenticationFailed,
            ),
            (
                "auth failed",
                zbus::fdo::Error::AuthFailed("x".to_owned()),
                AuthenticationFailed,
            ),
            (
                "failed",
                zbus::fdo::Error::Failed("x".to_owned()),
                AuthenticationFailed,
            ),
        ];
        for (case, error, kind) in property_cases {
            assert_error(trust_property_error(error), kind, message, case);
        }
    }

    #[test]
    fn incoming_pairing_and_agent_registration_errors_map_to_typed_kinds() {
        use ErrorKind::*;
        let message = "failed to open the pairing window";
        let cases = [
            ("org.bluez.Error.DoesNotExist", MissingAdapter),
            ("org.freedesktop.DBus.Error.UnknownObject", MissingAdapter),
            ("org.bluez.Error.NotAuthorized", AuthenticationFailed),
            (
                "org.freedesktop.DBus.Error.AccessDenied",
                AuthenticationFailed,
            ),
            ("org.bluez.Error.Failed", CapabilityUnavailable),
        ];
        for (name, kind) in cases {
            let error = incoming_pairing_error("open the pairing window", method_error(name));
            assert_error(error, kind, message, name);
        }
        let error = incoming_pairing_error(
            "open the pairing window",
            zbus::Error::Failure("x".to_owned()),
        );
        assert_error(error, CapabilityUnavailable, message, "transport failure");

        let property_cases = [
            (
                "unknown object",
                zbus::fdo::Error::UnknownObject("x".to_owned()),
                MissingAdapter,
            ),
            (
                "access denied",
                zbus::fdo::Error::AccessDenied("x".to_owned()),
                AuthenticationFailed,
            ),
            (
                "auth failed",
                zbus::fdo::Error::AuthFailed("x".to_owned()),
                AuthenticationFailed,
            ),
            (
                "failed",
                zbus::fdo::Error::Failed("x".to_owned()),
                CapabilityUnavailable,
            ),
        ];
        for (case, error, kind) in property_cases {
            let error = incoming_pairing_property_error("open the pairing window", error);
            assert_error(error, kind, message, case);
        }

        let error = pairing_agent_registration_error("register the agent", "bus refused");
        assert_error(
            error.clone(),
            CapabilityUnavailable,
            "failed to register the agent",
            "registration",
        );
        assert_eq!(error.diagnostic(), Some("bus refused"));
    }

    /// The outcome of every device-taking agent callback, as `Ok(())` or the rejection text.
    fn callback_outcomes(
        agent: &PairingAgent,
        device: &str,
    ) -> [(&'static str, Result<(), String>); 5] {
        let path = || OwnedObjectPath::try_from(device).unwrap();
        let text = |error: PairingAgentError| {
            let PairingAgentError::Rejected(message) = error;
            message
        };
        [
            (
                "request_pin_code",
                agent.request_pin_code(path()).map(drop).map_err(text),
            ),
            (
                "request_passkey",
                agent.request_passkey(path()).map(drop).map_err(text),
            ),
            (
                "request_confirmation",
                agent.request_confirmation(path(), 123_456).map_err(text),
            ),
            (
                "request_authorization",
                agent.request_authorization(path()).map_err(text),
            ),
            (
                "authorize_service",
                agent
                    .authorize_service(path(), "0000110a-0000-1000-8000-00805f9b34fb".to_owned())
                    .map_err(text),
            ),
        ]
    }

    fn assert_all_refused(agent: &PairingAgent, device: &str, case: &str) {
        for (callback, outcome) in callback_outcomes(agent, device) {
            assert_eq!(outcome, Err(UNAUTHORIZED.to_owned()), "{case}: {callback}");
        }
    }

    fn assert_authorized(agent: &PairingAgent, device: &str, case: &str) {
        let expected = [
            Err("BlueRoute NoInputNoOutput pairing cannot provide a PIN code".to_owned()),
            Err("BlueRoute NoInputNoOutput pairing cannot provide a passkey".to_owned()),
            Err("BlueRoute NoInputNoOutput pairing cannot confirm a displayed passkey".to_owned()),
            Ok(()),
            Ok(()),
        ];
        for ((callback, outcome), expected) in
            callback_outcomes(agent, device).into_iter().zip(expected)
        {
            assert_eq!(outcome, expected, "{case}: {callback}");
        }
    }

    #[test]
    fn every_agent_callback_refuses_devices_outside_an_active_permit() {
        let control = Arc::new(PairingControl::default());
        let agent = PairingAgent {
            control: Arc::clone(&control),
        };
        assert_all_refused(&agent, DEVICE, "no pairing operation");

        let permit = control.begin(&PeerHandle::new(DEVICE).unwrap()).unwrap();
        assert_authorized(&agent, DEVICE, "permitted peer");
        assert_all_refused(&agent, OTHER_DEVICE, "other peer during a permit");
        assert_all_refused(
            &agent,
            OTHER_ADAPTER_DEVICE,
            "other adapter during a permit",
        );
        assert_all_refused(&agent, NESTED_PATH, "child object of the permitted peer");
        drop(permit);
        assert_all_refused(&agent, DEVICE, "after the permit is dropped");
    }

    #[test]
    fn every_agent_callback_refuses_devices_outside_the_incoming_window() {
        let control = Arc::new(PairingControl::default());
        let agent = PairingAgent {
            control: Arc::clone(&control),
        };
        let adapter = AdapterHandle::new("/org/bluez/hci0").unwrap();
        control.begin_incoming(&adapter).unwrap();
        assert_authorized(&agent, DEVICE, "device on the window's adapter");
        assert_authorized(
            &agent,
            OTHER_DEVICE,
            "second device on the window's adapter",
        );
        assert_all_refused(&agent, OTHER_ADAPTER_DEVICE, "device on another adapter");
        assert_all_refused(&agent, NESTED_PATH, "child object below a device");
        assert_all_refused(&agent, "/org/bluez/hci0", "the adapter itself");
        assert_all_refused(
            &agent,
            "/org/bluez/hci0/other_AA",
            "non-device child of the adapter",
        );
        assert_all_refused(
            &agent,
            "/org/bluez/hci00/dev_AA_BB_CC_DD_EE_FF",
            "adapter name prefix",
        );
        control.end_incoming(&adapter).unwrap();
        assert_all_refused(&agent, DEVICE, "after the window ends");
    }
}

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_io::Timer;
use futures_lite::{StreamExt, future::race};
use zbus::fdo::{DBusProxy, ManagedObjects, ObjectManagerProxy};
use zbus::message::Type as MessageType;
use zbus::names::{BusName, OwnedInterfaceName};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, Message, MessageStream, Proxy};

use blueroute_core::{CoreError, ErrorKind};

use crate::{
    AdapterEventSubscription, AdapterHandle, BackendFuture, BluetoothAdapter,
    BluetoothAdapterEvent, BluetoothBackend, BluetoothPeerEvent, DiscoveredPeer,
    IncomingPairingWindow, PeerEventSubscription, PeerHandle,
};

const BLUEZ_SERVICE: &str = "org.bluez";
const BLUEZ_ROOT_PATH: &str = "/";
const BLUEZ_OBJECT_PREFIX: &str = "/org/bluez/";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const DEVICE_INTERFACE: &str = "org.bluez.Device1";
const AGENT_MANAGER_INTERFACE: &str = "org.bluez.AgentManager1";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const INTERFACES_ADDED: &str = "InterfacesAdded";
const INTERFACES_REMOVED: &str = "InterfacesRemoved";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const POWERED_PROPERTY: &str = "Powered";
const PAIRABLE_PROPERTY: &str = "Pairable";
const DISCOVERABLE_PROPERTY: &str = "Discoverable";
const ALIAS_PROPERTY: &str = "Alias";
const NAME_PROPERTY: &str = "Name";
const PAIRED_PROPERTY: &str = "Paired";
const TRUSTED_PROPERTY: &str = "Trusted";
const START_DISCOVERY_METHOD: &str = "StartDiscovery";
const STOP_DISCOVERY_METHOD: &str = "StopDiscovery";
const PAIR_METHOD: &str = "Pair";
const CANCEL_PAIRING_METHOD: &str = "CancelPairing";
const REGISTER_AGENT_METHOD: &str = "RegisterAgent";
const UNREGISTER_AGENT_METHOD: &str = "UnregisterAgent";
const REQUEST_DEFAULT_AGENT_METHOD: &str = "RequestDefaultAgent";
const PAIRING_AGENT_PATH: &str = "/org/blueroute/PairingAgent";
const PAIRING_AGENT_CAPABILITY: &str = "NoInputNoOutput";
const PAIRING_TIMEOUT: Duration = Duration::from_secs(60);

/// Production BlueZ backend using the Linux system D-Bus directly.
#[derive(Clone, Debug)]
pub struct BluezBackend {
    connection: Connection,
    pairing: Arc<PairingControl>,
}

impl BluezBackend {
    /// Connects to the system bus and verifies that `org.bluez` currently has an owner.
    pub async fn connect_system() -> Result<Self, CoreError> {
        let connection = Connection::system()
            .await
            .map_err(|error| bluez_error("failed to connect to the system D-Bus", error))?;
        ensure_bluez_available(&connection).await?;
        Ok(Self {
            connection,
            pairing: Arc::new(PairingControl::default()),
        })
    }

    /// Returns whether the BlueZ service currently owns its well-known bus name.
    pub async fn service_available(&self) -> Result<bool, CoreError> {
        bluez_service_available(&self.connection).await
    }

    async fn snapshot(&self) -> Result<Vec<BluetoothAdapter>, CoreError> {
        enumerate_adapters(&self.connection).await
    }
}

impl BluetoothBackend for BluezBackend {
    fn adapters(&self) -> BackendFuture<'_, Vec<BluetoothAdapter>> {
        Box::pin(async move { self.snapshot().await })
    }

    fn subscribe_adapter_events(&self) -> BackendFuture<'_, Box<dyn AdapterEventSubscription>> {
        Box::pin(async move {
            ensure_bluez_available(&self.connection).await?;
            let stream = bluez_signal_stream(&self.connection).await?;
            let snapshot = self.snapshot().await?;
            Ok(Box::new(BluezAdapterSubscription {
                connection: self.connection.clone(),
                stream,
                snapshot,
                pending: VecDeque::new(),
            }) as Box<dyn AdapterEventSubscription>)
        })
    }

    fn start_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            call_discovery_method(&self.connection, &adapter, START_DISCOVERY_METHOD).await
        })
    }

    fn stop_discovery(&self, adapter: AdapterHandle) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            call_discovery_method(&self.connection, &adapter, STOP_DISCOVERY_METHOD).await
        })
    }

    fn discovered_peers(&self, adapter: AdapterHandle) -> BackendFuture<'_, Vec<DiscoveredPeer>> {
        Box::pin(async move { enumerate_peers(&self.connection, &adapter).await })
    }

    fn subscribe_peer_events(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, Box<dyn PeerEventSubscription>> {
        Box::pin(async move {
            ensure_bluez_available(&self.connection).await?;
            let stream = bluez_signal_stream(&self.connection).await?;
            ensure_adapter_exists(&self.connection, &adapter).await?;
            Ok(Box::new(BluezPeerSubscription {
                connection: self.connection.clone(),
                stream,
                adapter,
            }) as Box<dyn PeerEventSubscription>)
        })
    }

    fn begin_incoming_pairing(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, IncomingPairingWindow> {
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move { begin_incoming_pairing(&self.connection, &pairing, &adapter).await })
    }

    fn end_incoming_pairing(&self, window: IncomingPairingWindow) -> BackendFuture<'_, ()> {
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move { end_incoming_pairing(&self.connection, &pairing, window).await })
    }

    fn pair(&self, peer: PeerHandle) -> BackendFuture<'_, ()> {
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move { pair_peer(&self.connection, &pairing, &peer).await })
    }

    fn set_trusted(&self, peer: PeerHandle, trusted: bool) -> BackendFuture<'_, ()> {
        Box::pin(async move { set_peer_trusted(&self.connection, &peer, trusted).await })
    }
}

#[derive(Debug, Default)]
struct PairingControl {
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
        BLUEZ_ROOT_PATH,
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
        BLUEZ_ROOT_PATH,
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
        BLUEZ_ROOT_PATH,
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

async fn begin_incoming_pairing(
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

async fn end_incoming_pairing(
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

async fn pair_peer(
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

async fn set_peer_trusted(
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

struct BluezAdapterSubscription {
    connection: Connection,
    stream: MessageStream,
    snapshot: Vec<BluetoothAdapter>,
    pending: VecDeque<BluetoothAdapterEvent>,
}

impl AdapterEventSubscription for BluezAdapterSubscription {
    fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothAdapterEvent>> {
        Box::pin(async move {
            loop {
                if let Some(event) = self.pending.pop_front() {
                    return Ok(Some(event));
                }

                let Some(message) = self.stream.next().await else {
                    return Ok(None);
                };
                let message = message.map_err(|error| {
                    bluez_error("failed while receiving BlueZ adapter changes", error)
                })?;
                if !is_adapter_change_signal(&message)? {
                    continue;
                }

                let next = enumerate_adapters(&self.connection).await?;
                self.pending = diff_adapter_snapshots(&self.snapshot, &next);
                self.snapshot = next;
            }
        })
    }
}

struct BluezPeerSubscription {
    connection: Connection,
    stream: MessageStream,
    adapter: AdapterHandle,
}

impl PeerEventSubscription for BluezPeerSubscription {
    fn next_event(&mut self) -> BackendFuture<'_, Option<BluetoothPeerEvent>> {
        Box::pin(async move {
            loop {
                let Some(message) = self.stream.next().await else {
                    return Ok(None);
                };
                let message = message.map_err(|error| {
                    bluez_error("failed while receiving BlueZ peer changes", error)
                })?;
                if let Some(event) =
                    peer_event_from_signal(&self.connection, &self.adapter, &message).await?
                {
                    return Ok(Some(event));
                }
            }
        })
    }
}

async fn bluez_service_available(connection: &Connection) -> Result<bool, CoreError> {
    let proxy = DBusProxy::new(connection)
        .await
        .map_err(|error| bluez_error("failed to create a system D-Bus service proxy", error))?;
    let service_name = BusName::try_from(BLUEZ_SERVICE).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::Internal,
            "BlueZ service name is invalid",
            error.to_string(),
        )
    })?;
    proxy
        .name_has_owner(service_name)
        .await
        .map_err(|error| bluez_error("failed to query BlueZ service availability", error))
}

async fn ensure_bluez_available(connection: &Connection) -> Result<(), CoreError> {
    if bluez_service_available(connection).await? {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::BluezUnavailable,
            "BlueZ is not available on the system D-Bus",
        ))
    }
}

async fn managed_objects(connection: &Connection) -> Result<ManagedObjects, CoreError> {
    ensure_bluez_available(connection).await?;
    let proxy = ObjectManagerProxy::new(connection, BLUEZ_SERVICE, BLUEZ_ROOT_PATH)
        .await
        .map_err(|error| bluez_error("failed to create the BlueZ object-manager proxy", error))?;
    proxy
        .get_managed_objects()
        .await
        .map_err(|error| bluez_error("failed to enumerate BlueZ managed objects", error))
}

async fn enumerate_adapters(connection: &Connection) -> Result<Vec<BluetoothAdapter>, CoreError> {
    let objects = managed_objects(connection).await?;
    adapters_from_managed_objects(&objects)
}

async fn ensure_adapter_exists(
    connection: &Connection,
    adapter: &AdapterHandle,
) -> Result<BluetoothAdapter, CoreError> {
    enumerate_adapters(connection)
        .await?
        .into_iter()
        .find(|candidate| candidate.handle == *adapter)
        .ok_or_else(|| {
            CoreError::new(
                ErrorKind::MissingAdapter,
                "Bluetooth adapter is no longer available",
            )
        })
}

async fn call_discovery_method(
    connection: &Connection,
    adapter: &AdapterHandle,
    method: &'static str,
) -> Result<(), CoreError> {
    let current = ensure_adapter_exists(connection, adapter).await?;
    if method == START_DISCOVERY_METHOD && !current.powered {
        return Err(CoreError::new(
            ErrorKind::AdapterDisabled,
            "Bluetooth adapter must be powered before discovery can start",
        ));
    }

    let proxy = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        current.handle.as_str(),
        ADAPTER_INTERFACE,
    )
    .await
    .map_err(|error| discovery_error(method, error))?;
    proxy
        .call_method(method, &())
        .await
        .map_err(|error| discovery_error(method, error))?;
    Ok(())
}

fn discovery_error(method: &'static str, error: zbus::Error) -> CoreError {
    let kind = match &error {
        zbus::Error::MethodError(name, _, _) => discovery_method_error_kind(name.as_str()),
        _ => ErrorKind::BluezUnavailable,
    };
    let operation = if method == START_DISCOVERY_METHOD {
        "start Bluetooth discovery"
    } else {
        "stop Bluetooth discovery"
    };
    CoreError::with_diagnostic(kind, format!("failed to {operation}"), error.to_string())
}

fn discovery_method_error_kind(name: &str) -> ErrorKind {
    match name {
        "org.bluez.Error.NotReady" => ErrorKind::AdapterDisabled,
        "org.bluez.Error.InProgress" => ErrorKind::InvalidState,
        "org.bluez.Error.NotAuthorized" => ErrorKind::CapabilityUnavailable,
        "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject" => {
            ErrorKind::MissingAdapter
        }
        _ => ErrorKind::CapabilityUnavailable,
    }
}

async fn enumerate_peers(
    connection: &Connection,
    adapter: &AdapterHandle,
) -> Result<Vec<DiscoveredPeer>, CoreError> {
    let objects = managed_objects(connection).await?;
    let adapter_exists = adapters_from_managed_objects(&objects)?
        .into_iter()
        .any(|candidate| candidate.handle == *adapter);
    if !adapter_exists {
        return Err(CoreError::new(
            ErrorKind::MissingAdapter,
            "Bluetooth adapter is no longer available",
        ));
    }
    peers_from_managed_objects(&objects, adapter)
}

fn adapters_from_managed_objects(
    objects: &ManagedObjects,
) -> Result<Vec<BluetoothAdapter>, CoreError> {
    let mut adapters = Vec::new();
    for (path, interfaces) in objects {
        let Some(properties) = interfaces.iter().find_map(|(name, properties)| {
            (name.as_str() == ADAPTER_INTERFACE).then_some(properties)
        }) else {
            continue;
        };

        let powered = properties
            .get(POWERED_PROPERTY)
            .ok_or_else(|| {
                CoreError::new(
                    ErrorKind::ProtocolError,
                    "BlueZ adapter is missing its Powered property",
                )
            })
            .and_then(owned_value_to_bool)?;
        let handle = AdapterHandle::new(path.as_str())?;
        adapters.push(BluetoothAdapter { handle, powered });
    }
    adapters.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(adapters)
}

fn peers_from_managed_objects(
    objects: &ManagedObjects,
    adapter: &AdapterHandle,
) -> Result<Vec<DiscoveredPeer>, CoreError> {
    let mut peers = Vec::new();
    for (path, interfaces) in objects {
        if !is_device_object_path_for_adapter(path.as_str(), adapter) {
            continue;
        }
        let Some(properties) = interfaces.iter().find_map(|(name, properties)| {
            (name.as_str() == DEVICE_INTERFACE).then_some(properties)
        }) else {
            continue;
        };
        peers.push(peer_from_properties(path, properties)?);
    }
    peers.sort_by(|left, right| left.handle.cmp(&right.handle));
    Ok(peers)
}

fn peer_from_properties(
    path: &OwnedObjectPath,
    properties: &HashMap<String, OwnedValue>,
) -> Result<DiscoveredPeer, CoreError> {
    let paired = required_peer_bool(properties, PAIRED_PROPERTY)?;
    let trusted = required_peer_bool(properties, TRUSTED_PROPERTY)?;
    let display_name = optional_peer_string(properties, ALIAS_PROPERTY)?
        .or(optional_peer_string(properties, NAME_PROPERTY)?);
    Ok(DiscoveredPeer {
        handle: PeerHandle::new(path.as_str())?,
        display_name,
        paired,
        trusted,
    })
}

fn required_peer_bool(
    properties: &HashMap<String, OwnedValue>,
    property: &'static str,
) -> Result<bool, CoreError> {
    let value = properties.get(property).ok_or_else(|| {
        CoreError::new(
            ErrorKind::ProtocolError,
            format!("BlueZ device is missing its {property} property"),
        )
    })?;
    bool::try_from(value).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            format!("BlueZ device {property} property is not a boolean"),
            error.to_string(),
        )
    })
}

fn optional_peer_string(
    properties: &HashMap<String, OwnedValue>,
    property: &'static str,
) -> Result<Option<String>, CoreError> {
    let Some(value) = properties.get(property) else {
        return Ok(None);
    };
    let value = <&str>::try_from(value).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            format!("BlueZ device {property} property is not a string"),
            error.to_string(),
        )
    })?;
    if value.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(value.to_owned()))
    }
}

fn is_device_object_path_for_adapter(path: &str, adapter: &AdapterHandle) -> bool {
    is_device_object_path_for_adapter_path(path, adapter.as_str())
}

fn is_device_object_path_for_adapter_path(path: &str, adapter: &str) -> bool {
    let prefix = format!("{}/", adapter.trim_end_matches('/'));
    let Some(suffix) = path.strip_prefix(&prefix) else {
        return false;
    };
    suffix.starts_with("dev_") && !suffix.contains('/')
}

fn owned_value_to_bool(value: &OwnedValue) -> Result<bool, CoreError> {
    bool::try_from(value).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::ProtocolError,
            "BlueZ adapter Powered property is not a boolean",
            error.to_string(),
        )
    })
}

async fn bluez_signal_stream(connection: &Connection) -> Result<MessageStream, CoreError> {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BLUEZ_SERVICE)
        .map_err(|error| bluez_error("failed to build the BlueZ signal subscription", error))?
        .build();
    MessageStream::for_match_rule(rule, connection, Some(64))
        .await
        .map_err(|error| bluez_error("failed to subscribe to BlueZ changes", error))
}

fn is_adapter_change_signal(message: &Message) -> Result<bool, CoreError> {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str());
    let member = header.member().map(|name| name.as_str());

    match (interface, member) {
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_ADDED)) => {
            let (path, interfaces): (
                OwnedObjectPath,
                HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ InterfacesAdded signal", error)
            })?;
            Ok(is_adapter_object_path(path.as_str())
                && interfaces
                    .keys()
                    .any(|name| name.as_str() == ADAPTER_INTERFACE))
        }
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_REMOVED)) => {
            let (path, interfaces): (OwnedObjectPath, Vec<OwnedInterfaceName>) =
                message.body().deserialize().map_err(|error| {
                    bluez_error("failed to decode BlueZ InterfacesRemoved signal", error)
                })?;
            Ok(is_adapter_object_path(path.as_str())
                && interfaces
                    .iter()
                    .any(|name| name.as_str() == ADAPTER_INTERFACE))
        }
        (Some(PROPERTIES_INTERFACE), Some(PROPERTIES_CHANGED)) => {
            let Some(path) = header.path() else {
                return Ok(false);
            };
            if !is_adapter_object_path(path.as_str()) {
                return Ok(false);
            }
            let (interface_name, changed, invalidated): (
                OwnedInterfaceName,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ PropertiesChanged signal", error)
            })?;
            Ok(interface_name.as_str() == ADAPTER_INTERFACE
                && (changed.contains_key(POWERED_PROPERTY)
                    || invalidated.iter().any(|name| name == POWERED_PROPERTY)))
        }
        _ => Ok(false),
    }
}

async fn peer_event_from_signal(
    connection: &Connection,
    adapter: &AdapterHandle,
    message: &Message,
) -> Result<Option<BluetoothPeerEvent>, CoreError> {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str());
    let member = header.member().map(|name| name.as_str());

    match (interface, member) {
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_ADDED)) => {
            let (path, interfaces): (
                OwnedObjectPath,
                HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ InterfacesAdded signal", error)
            })?;
            if !is_device_object_path_for_adapter(path.as_str(), adapter) {
                return Ok(None);
            }
            let Some(properties) = interfaces.iter().find_map(|(name, properties)| {
                (name.as_str() == DEVICE_INTERFACE).then_some(properties)
            }) else {
                return Ok(None);
            };
            Ok(Some(BluetoothPeerEvent::Added(peer_from_properties(
                &path, properties,
            )?)))
        }
        (Some(OBJECT_MANAGER_INTERFACE), Some(INTERFACES_REMOVED)) => {
            let (path, interfaces): (OwnedObjectPath, Vec<OwnedInterfaceName>) =
                message.body().deserialize().map_err(|error| {
                    bluez_error("failed to decode BlueZ InterfacesRemoved signal", error)
                })?;
            if !is_device_object_path_for_adapter(path.as_str(), adapter)
                || !interfaces
                    .iter()
                    .any(|name| name.as_str() == DEVICE_INTERFACE)
            {
                return Ok(None);
            }
            Ok(Some(BluetoothPeerEvent::Removed(PeerHandle::new(
                path.as_str(),
            )?)))
        }
        (Some(PROPERTIES_INTERFACE), Some(PROPERTIES_CHANGED)) => {
            let Some(path) = header.path() else {
                return Ok(None);
            };
            if !is_device_object_path_for_adapter(path.as_str(), adapter) {
                return Ok(None);
            }
            let (interface_name, changed, invalidated): (
                OwnedInterfaceName,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize().map_err(|error| {
                bluez_error("failed to decode BlueZ PropertiesChanged signal", error)
            })?;
            if interface_name.as_str() != DEVICE_INTERFACE
                || !peer_properties_changed(&changed, &invalidated)
            {
                return Ok(None);
            }
            let Some(peer) = fetch_peer(connection, adapter, path.as_str()).await? else {
                return Ok(None);
            };
            Ok(Some(BluetoothPeerEvent::Changed(peer)))
        }
        _ => Ok(None),
    }
}

fn peer_properties_changed(changed: &HashMap<String, OwnedValue>, invalidated: &[String]) -> bool {
    [
        ALIAS_PROPERTY,
        NAME_PROPERTY,
        PAIRED_PROPERTY,
        TRUSTED_PROPERTY,
    ]
    .iter()
    .any(|property| {
        changed.contains_key(*property)
            || invalidated
                .iter()
                .any(|invalid| invalid.as_str() == *property)
    })
}

async fn fetch_peer(
    connection: &Connection,
    adapter: &AdapterHandle,
    path: &str,
) -> Result<Option<DiscoveredPeer>, CoreError> {
    let objects = managed_objects(connection).await?;
    let Some((object_path, interfaces)) = objects
        .iter()
        .find(|(candidate, _)| candidate.as_str() == path)
    else {
        return Ok(None);
    };
    if !is_device_object_path_for_adapter(object_path.as_str(), adapter) {
        return Ok(None);
    }
    let Some(properties) = interfaces
        .iter()
        .find_map(|(name, properties)| (name.as_str() == DEVICE_INTERFACE).then_some(properties))
    else {
        return Ok(None);
    };
    peer_from_properties(object_path, properties).map(Some)
}

fn is_adapter_object_path(path: &str) -> bool {
    let Some(suffix) = path.strip_prefix(BLUEZ_OBJECT_PREFIX) else {
        return false;
    };
    !suffix.is_empty() && !suffix.contains('/')
}

fn diff_adapter_snapshots(
    previous: &[BluetoothAdapter],
    current: &[BluetoothAdapter],
) -> VecDeque<BluetoothAdapterEvent> {
    let previous = previous
        .iter()
        .map(|adapter| (adapter.handle.clone(), adapter))
        .collect::<BTreeMap<_, _>>();
    let current = current
        .iter()
        .map(|adapter| (adapter.handle.clone(), adapter))
        .collect::<BTreeMap<_, _>>();
    let mut events = VecDeque::new();

    for (handle, old) in &previous {
        if !current.contains_key(handle) {
            events.push_back(BluetoothAdapterEvent::Removed(handle.clone()));
        } else if let Some(new) = current.get(handle)
            && old.powered != new.powered
        {
            events.push_back(BluetoothAdapterEvent::PoweredChanged {
                handle: handle.clone(),
                powered: new.powered,
            });
        }
    }
    for (handle, adapter) in &current {
        if !previous.contains_key(handle) {
            events.push_back(BluetoothAdapterEvent::Added((*adapter).clone()));
        }
    }
    events
}

fn bluez_error(message: &'static str, error: impl std::fmt::Display) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::BluezUnavailable, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Str;

    fn adapter(path: &str, powered: bool) -> BluetoothAdapter {
        BluetoothAdapter {
            handle: AdapterHandle::new(path).unwrap(),
            powered,
        }
    }

    fn peer_properties(
        alias: Option<&str>,
        name: Option<&str>,
        paired: bool,
        trusted: bool,
    ) -> HashMap<String, OwnedValue> {
        let mut properties = HashMap::new();
        if let Some(alias) = alias {
            properties.insert(
                ALIAS_PROPERTY.to_owned(),
                OwnedValue::from(Str::from(alias)),
            );
        }
        if let Some(name) = name {
            properties.insert(NAME_PROPERTY.to_owned(), OwnedValue::from(Str::from(name)));
        }
        properties.insert(PAIRED_PROPERTY.to_owned(), OwnedValue::from(paired));
        properties.insert(TRUSTED_PROPERTY.to_owned(), OwnedValue::from(trusted));
        properties
    }

    fn add_device(
        objects: &mut ManagedObjects,
        path: &str,
        properties: HashMap<String, OwnedValue>,
    ) {
        let mut interfaces = HashMap::new();
        interfaces.insert(
            OwnedInterfaceName::try_from(DEVICE_INTERFACE).unwrap(),
            properties,
        );
        objects.insert(OwnedObjectPath::try_from(path).unwrap(), interfaces);
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
        assert!(!is_device_object_path_for_adapter(
            "/org/bluez/hci0/not_a_device",
            &hci0
        ));
    }

    #[test]
    fn peer_mapping_prefers_alias_and_falls_back_to_name() {
        let path = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let aliased = peer_from_properties(
            &path,
            &peer_properties(Some("Friendly"), Some("Remote Name"), true, false),
        )
        .unwrap();
        assert_eq!(aliased.display_name.as_deref(), Some("Friendly"));
        assert!(aliased.paired);
        assert!(!aliased.trusted);

        let named = peer_from_properties(
            &path,
            &peer_properties(None, Some("Remote Name"), false, true),
        )
        .unwrap();
        assert_eq!(named.display_name.as_deref(), Some("Remote Name"));
        assert!(!named.paired);
        assert!(named.trusted);
    }

    #[test]
    fn peer_mapping_rejects_missing_required_boolean_properties() {
        let path = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let mut properties = peer_properties(Some("Peer"), None, false, false);
        properties.remove(PAIRED_PROPERTY);
        let error = peer_from_properties(&path, &properties).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ProtocolError);
        assert!(error.message().contains(PAIRED_PROPERTY));
    }

    #[test]
    fn peer_snapshot_filters_adapter_and_sorts_by_stable_handle() {
        let mut objects = ManagedObjects::new();
        add_device(
            &mut objects,
            "/org/bluez/hci0/dev_BB_BB_BB_BB_BB_BB",
            peer_properties(Some("Second"), None, false, false),
        );
        add_device(
            &mut objects,
            "/org/bluez/hci1/dev_00_00_00_00_00_00",
            peer_properties(Some("Other Adapter"), None, false, false),
        );
        add_device(
            &mut objects,
            "/org/bluez/hci0/dev_AA_AA_AA_AA_AA_AA",
            peer_properties(Some("First"), None, true, true),
        );

        let peers =
            peers_from_managed_objects(&objects, &AdapterHandle::new("/org/bluez/hci0").unwrap())
                .unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(
            peers[0].handle.as_str(),
            "/org/bluez/hci0/dev_AA_AA_AA_AA_AA_AA"
        );
        assert_eq!(
            peers[1].handle.as_str(),
            "/org/bluez/hci0/dev_BB_BB_BB_BB_BB_BB"
        );
    }

    #[test]
    fn discovery_method_errors_map_to_typed_core_errors() {
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.NotReady"),
            ErrorKind::AdapterDisabled
        );
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.InProgress"),
            ErrorKind::InvalidState
        );
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.DoesNotExist"),
            ErrorKind::MissingAdapter
        );
        assert_eq!(
            discovery_method_error_kind("org.bluez.Error.NotAuthorized"),
            ErrorKind::CapabilityUnavailable
        );
    }

    #[test]
    fn peer_change_filter_ignores_unmodeled_high_rate_properties() {
        let mut changed = HashMap::new();
        changed.insert("RSSI".to_owned(), OwnedValue::from(-42_i16));
        assert!(!peer_properties_changed(&changed, &[]));

        changed.insert(PAIRED_PROPERTY.to_owned(), OwnedValue::from(true));
        assert!(peer_properties_changed(&changed, &[]));
        assert!(peer_properties_changed(
            &HashMap::new(),
            &[ALIAS_PROPERTY.to_owned()]
        ));
    }

    #[test]
    fn adapter_object_paths_exclude_nested_device_objects() {
        assert!(is_adapter_object_path("/org/bluez/hci0"));
        assert!(is_adapter_object_path("/org/bluez/controller-name"));
        assert!(!is_adapter_object_path(
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
        ));
        assert!(!is_adapter_object_path("/org/bluez/"));
        assert!(!is_adapter_object_path("/other/hci0"));
    }

    #[test]
    fn adapter_snapshot_diff_reports_remove_power_and_add_deterministically() {
        let previous = vec![
            adapter("/org/bluez/hci0", false),
            adapter("/org/bluez/hci1", true),
        ];
        let current = vec![
            adapter("/org/bluez/hci0", true),
            adapter("/org/bluez/hci2", false),
        ];

        let events = diff_adapter_snapshots(&previous, &current)
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            vec![
                BluetoothAdapterEvent::PoweredChanged {
                    handle: AdapterHandle::new("/org/bluez/hci0").unwrap(),
                    powered: true,
                },
                BluetoothAdapterEvent::Removed(AdapterHandle::new("/org/bluez/hci1").unwrap()),
                BluetoothAdapterEvent::Added(adapter("/org/bluez/hci2", false)),
            ]
        );
    }

    #[test]
    fn unchanged_snapshot_produces_no_events() {
        let snapshot = vec![adapter("/org/bluez/hci0", true)];
        assert!(diff_adapter_snapshots(&snapshot, &snapshot).is_empty());
    }

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
}

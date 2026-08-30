from pathlib import Path

lib = Path('crates/blueroute-linux/src/lib.rs')
text = lib.read_text()
anchor = '''#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredPeer {
    pub handle: PeerHandle,
    pub display_name: Option<String>,
    pub paired: bool,
    pub trusted: bool,
}
'''
insert = anchor + '''
/// Opaque restore token for a bounded incoming Bluetooth pairing window.
#[derive(Debug, Eq, PartialEq)]
pub struct IncomingPairingWindow {
    pub(crate) adapter: AdapterHandle,
    pub(crate) restore_discoverable: bool,
    pub(crate) restore_pairable: bool,
}

impl IncomingPairingWindow {
    pub fn adapter(&self) -> &AdapterHandle {
        &self.adapter
    }
}
'''
if anchor not in text:
    raise SystemExit('DiscoveredPeer anchor not found')
text = text.replace(anchor, insert, 1)
old = '''    fn subscribe_peer_events(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, Box<dyn PeerEventSubscription>>;
    fn pair(&self, peer: PeerHandle) -> BackendFuture<'_, ()>;
'''
new = '''    fn subscribe_peer_events(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, Box<dyn PeerEventSubscription>>;
    fn begin_incoming_pairing(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, IncomingPairingWindow>;
    fn end_incoming_pairing(&self, window: IncomingPairingWindow) -> BackendFuture<'_, ()>;
    fn pair(&self, peer: PeerHandle) -> BackendFuture<'_, ()>;
'''
if old not in text:
    raise SystemExit('BluetoothBackend method anchor not found')
text = text.replace(old, new, 1)
old_fake = '''        fn pair(&self, _peer: PeerHandle) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
'''
new_fake = '''        fn begin_incoming_pairing(
            &self,
            adapter: AdapterHandle,
        ) -> BackendFuture<'_, IncomingPairingWindow> {
            Box::pin(async move {
                Ok(IncomingPairingWindow {
                    adapter,
                    restore_discoverable: false,
                    restore_pairable: false,
                })
            })
        }

        fn end_incoming_pairing(&self, _window: IncomingPairingWindow) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn pair(&self, _peer: PeerHandle) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
'''
if old_fake not in text:
    raise SystemExit('FakeBluetooth pair anchor not found')
text = text.replace(old_fake, new_fake, 1)
old_test = '''        let mut peers = resolve(backend.subscribe_peer_events(adapters[0].handle.clone())).unwrap();
        assert_eq!(resolve(peers.next_event()).unwrap(), None);
'''
new_test = '''        let mut peers = resolve(backend.subscribe_peer_events(adapters[0].handle.clone())).unwrap();
        assert_eq!(resolve(peers.next_event()).unwrap(), None);
        let window = resolve(backend.begin_incoming_pairing(adapters[0].handle.clone())).unwrap();
        assert_eq!(window.adapter(), &adapters[0].handle);
        resolve(backend.end_incoming_pairing(window)).unwrap();
'''
if old_test not in text:
    raise SystemExit('fake boundary test anchor not found')
text = text.replace(old_test, new_test, 1)
lib.write_text(text)

bluez = Path('crates/blueroute-linux/src/bluez.rs')
text = bluez.read_text()
text = text.replace(
    '''    BluetoothAdapterEvent, BluetoothBackend, BluetoothPeerEvent, DiscoveredPeer,
    PeerEventSubscription, PeerHandle,
''',
    '''    BluetoothAdapterEvent, BluetoothBackend, BluetoothPeerEvent, DiscoveredPeer,
    IncomingPairingWindow, PeerEventSubscription, PeerHandle,
''',
    1,
)
text = text.replace(
    '''const POWERED_PROPERTY: &str = "Powered";
const ALIAS_PROPERTY: &str = "Alias";
''',
    '''const POWERED_PROPERTY: &str = "Powered";
const PAIRABLE_PROPERTY: &str = "Pairable";
const DISCOVERABLE_PROPERTY: &str = "Discoverable";
const ALIAS_PROPERTY: &str = "Alias";
''',
    1,
)
text = text.replace(
    '''const REGISTER_AGENT_METHOD: &str = "RegisterAgent";
const PAIRING_AGENT_PATH: &str = "/org/blueroute/PairingAgent";
''',
    '''const REGISTER_AGENT_METHOD: &str = "RegisterAgent";
const UNREGISTER_AGENT_METHOD: &str = "UnregisterAgent";
const REQUEST_DEFAULT_AGENT_METHOD: &str = "RequestDefaultAgent";
const PAIRING_AGENT_PATH: &str = "/org/blueroute/PairingAgent";
''',
    1,
)
old_methods = '''    fn pair(&self, peer: PeerHandle) -> BackendFuture<'_, ()> {
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move { pair_peer(&self.connection, &pairing, &peer).await })
    }
'''
new_methods = '''    fn begin_incoming_pairing(
        &self,
        adapter: AdapterHandle,
    ) -> BackendFuture<'_, IncomingPairingWindow> {
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move {
            begin_incoming_pairing(&self.connection, &pairing, &adapter).await
        })
    }

    fn end_incoming_pairing(&self, window: IncomingPairingWindow) -> BackendFuture<'_, ()> {
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move { end_incoming_pairing(&self.connection, &pairing, window).await })
    }

    fn pair(&self, peer: PeerHandle) -> BackendFuture<'_, ()> {
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move { pair_peer(&self.connection, &pairing, &peer).await })
    }
'''
if old_methods not in text:
    raise SystemExit('BluezBackend pair method anchor not found')
text = text.replace(old_methods, new_methods, 1)
start = text.index('#[derive(Debug, Default)]\nstruct PairingControl')
end = text.index('\n#[derive(Clone, Debug)]\nstruct PairingAgent', start)
replacement = '''#[derive(Debug, Default)]
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
'''
text = text[:start] + replacement + text[end:]

anchor = '''fn pairing_agent_registration_error(
    operation: &'static str,
    error: impl std::fmt::Display,
) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::CapabilityUnavailable,
        format!("failed to {operation}"),
        error.to_string(),
    )
}
'''
insert = anchor + r'''

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
        .map_err(|error| incoming_pairing_property_error("read Bluetooth Pairable state", error))?;
    let restore_discoverable: bool = proxy
        .get_property(DISCOVERABLE_PROPERTY)
        .await
        .map_err(|error| {
            incoming_pairing_property_error("read Bluetooth Discoverable state", error)
        })?;

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
            ) => ErrorKind::MissingAdapter,
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.NotAuthorized" | "org.freedesktop.DBus.Error.AccessDenied"
            ) => ErrorKind::AuthenticationFailed,
        _ => ErrorKind::CapabilityUnavailable,
    };
    CoreError::with_diagnostic(kind, format!("failed to {operation}"), error.to_string())
}
'''
if anchor not in text:
    raise SystemExit('pairing registration error anchor not found')
text = text.replace(anchor, insert, 1)
old_path_fn = '''fn is_device_object_path_for_adapter(path: &str, adapter: &AdapterHandle) -> bool {
    let prefix = format!("{}/", adapter.as_str().trim_end_matches('/'));
    let Some(suffix) = path.strip_prefix(&prefix) else {
        return false;
    };
    suffix.starts_with("dev_") && !suffix.contains('/')
}
'''
new_path_fn = '''fn is_device_object_path_for_adapter(path: &str, adapter: &AdapterHandle) -> bool {
    is_device_object_path_for_adapter_path(path, adapter.as_str())
}

fn is_device_object_path_for_adapter_path(path: &str, adapter: &str) -> bool {
    let prefix = format!("{}/", adapter.trim_end_matches('/'));
    let Some(suffix) = path.strip_prefix(&prefix) else {
        return false;
    };
    suffix.starts_with("dev_") && !suffix.contains('/')
}
'''
if old_path_fn not in text:
    raise SystemExit('device path helper anchor not found')
text = text.replace(old_path_fn, new_path_fn, 1)
# Add tests before final brace by anchoring existing no-input test.
old_test = '''    fn pairing_input_requests_are_rejected_for_no_input_no_output_agent() {
        let control = Arc::new(PairingControl::default());
        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let _permit = control.begin(&peer).unwrap();
        let agent = PairingAgent { control };
        let path = OwnedObjectPath::try_from(peer.as_str()).unwrap();
        assert!(agent.request_pin_code(path.clone()).is_err());
        assert!(agent.request_passkey(path.clone()).is_err());
        assert!(agent.request_confirmation(path, 123456).is_err());
    }
'''
new_test = old_test + '''
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
        assert_eq!(control.begin(&peer).unwrap_err().kind(), ErrorKind::InvalidState);
        control.end_incoming(&adapter).unwrap();
        let _permit = control.begin(&peer).unwrap();
        assert_eq!(
            control.begin_incoming(&adapter).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
    }
'''
if old_test not in text:
    raise SystemExit('pairing input test anchor not found')
text = text.replace(old_test, new_test, 1)
bluez.write_text(text)

example = Path('crates/blueroute-linux/examples/bluez_pair_accept.rs')
example.write_text(r'''use std::error::Error;
use std::time::Duration;

use async_io::Timer;
use blueroute_linux::{BluetoothBackend, BluezBackend};
use futures_lite::future;

const PAIRING_WINDOW: Duration = Duration::from_secs(120);

fn main() -> Result<(), Box<dyn Error>> {
    future::block_on(async move {
        let backend = BluezBackend::connect_system().await?;
        let adapter = backend
            .adapters()
            .await?
            .into_iter()
            .find(|adapter| adapter.powered)
            .ok_or("no powered Bluetooth adapter is available")?;

        let window = backend.begin_incoming_pairing(adapter.handle.clone()).await?;
        println!("adapter: {}", window.adapter().as_str());
        println!(
            "Rust-controlled incoming pairing window open for {} seconds...",
            PAIRING_WINDOW.as_secs()
        );
        println!("Run bluez_pair_probe from the other Linux test node now.");
        Timer::after(PAIRING_WINDOW).await;
        backend.end_incoming_pairing(window).await?;

        let paired = backend
            .discovered_peers(adapter.handle)
            .await?
            .into_iter()
            .filter(|peer| peer.paired)
            .collect::<Vec<_>>();
        println!("pairing window closed; {} paired peer(s) visible", paired.len());
        for peer in paired {
            println!(
                "{}\tname={}\tpaired={}\ttrusted={}",
                peer.handle.as_str(),
                peer.display_name.as_deref().unwrap_or("<unknown>"),
                peer.paired,
                peer.trusted
            );
        }
        Ok::<(), Box<dyn Error>>(())
    })
}
''')

doc = Path('docs/P4-004-PAIRING.md')
text = doc.read_text()
needle = '''## Hardware acceptance probe

The probe below discovers a peer by exact BlueZ object path or exact display name, invokes the Rust pairing adapter, sets trust, and verifies the refreshed `Device1` state:

```bash
cargo run -p blueroute-linux --example bluez_pair_probe --locked -- debiancb1
```

The remote test node must be powered, discoverable/pairable as appropriate, and have an authentication agent capable of completing its side of the pairing exchange. The P4-004 task remains in progress until two Linux test nodes complete this Rust-initiated flow and the evidence is recorded.
'''
replacement = '''## Hardware acceptance probes

For a fully Rust-controlled two-node test, run the bounded incoming pairing window on the receiving Linux node first:

```bash
cargo run -p blueroute-linux --example bluez_pair_accept --locked
```

The acceptor temporarily registers BlueRoute as the BlueZ default `NoInputNoOutput` agent and enables `Pairable` and `Discoverable` on the selected adapter for 120 seconds. It authorizes only Device1 objects under that adapter, restores the adapter's previous pairable/discoverable values when the window closes, clears authorization before cleanup, and unregisters the BlueRoute agent so it does not remain the system-wide default.

Then, while that window is open, run the initiator from the other Linux node. The probe discovers a peer by exact BlueZ object path or exact display name, invokes the Rust pairing adapter, explicitly sets trust, and verifies the refreshed `Device1` state:

```bash
cargo run -p blueroute-linux --example bluez_pair_probe --locked -- debiancb1
```

`RequestDefaultAgent` may require additional system policy authorization on some Linux distributions. BlueRoute reports that as a typed capability/authentication failure rather than falling back to a graphical agent or `bluetoothctl`.

The P4-004 task remains in progress until two Linux test nodes complete this Rust-controlled acceptor/initiator flow and the evidence is recorded.
'''
if needle not in text:
    raise SystemExit('hardware acceptance docs anchor not found')
text = text.replace(needle, replacement, 1)
doc.write_text(text)

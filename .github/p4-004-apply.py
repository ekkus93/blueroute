from pathlib import Path

cargo = Path('crates/blueroute-linux/Cargo.toml')
text = cargo.read_text()
text = text.replace(
    'blueroute-core = { path = "../blueroute-core" }\nfutures-lite = "2.6.0"',
    'blueroute-core = { path = "../blueroute-core" }\nasync-io = "2.6.0"\nfutures-lite = "2.6.0"',
)
cargo.write_text(text)

path = Path('crates/blueroute-linux/src/bluez.rs')
text = path.read_text()
text = text.replace(
    'use std::collections::{BTreeMap, HashMap, VecDeque};\n\nuse futures_lite::StreamExt;',
    'use std::collections::{BTreeMap, HashMap, VecDeque};\nuse std::sync::{Arc, Mutex};\nuse std::time::Duration;\n\nuse async_io::Timer;\nuse futures_lite::{future::race, StreamExt};',
)
text = text.replace(
    'const DEVICE_INTERFACE: &str = "org.bluez.Device1";\nconst OBJECT_MANAGER_INTERFACE:',
    'const DEVICE_INTERFACE: &str = "org.bluez.Device1";\nconst AGENT_INTERFACE: &str = "org.bluez.Agent1";\nconst AGENT_MANAGER_INTERFACE: &str = "org.bluez.AgentManager1";\nconst OBJECT_MANAGER_INTERFACE:',
)
text = text.replace(
    'const STOP_DISCOVERY_METHOD: &str = "StopDiscovery";\n',
    'const STOP_DISCOVERY_METHOD: &str = "StopDiscovery";\nconst PAIR_METHOD: &str = "Pair";\nconst CANCEL_PAIRING_METHOD: &str = "CancelPairing";\nconst REGISTER_AGENT_METHOD: &str = "RegisterAgent";\nconst PAIRING_AGENT_PATH: &str = "/org/blueroute/PairingAgent";\nconst PAIRING_AGENT_CAPABILITY: &str = "NoInputNoOutput";\nconst PAIRING_TIMEOUT: Duration = Duration::from_secs(60);\n',
)
text = text.replace(
    'pub struct BluezBackend {\n    connection: Connection,\n}',
    'pub struct BluezBackend {\n    connection: Connection,\n    pairing: Arc<PairingControl>,\n}',
)
text = text.replace(
    '        ensure_bluez_available(&connection).await?;\n        Ok(Self { connection })',
    '        ensure_bluez_available(&connection).await?;\n        Ok(Self {\n            connection,\n            pairing: Arc::new(PairingControl::default()),\n        })',
)
text = text.replace(
    '''    fn pair(&self, _peer: PeerHandle) -> BackendFuture<'_, ()> {\n        unsupported_future("Bluetooth pairing is not implemented until P4-004")\n    }\n\n    fn set_trusted(&self, _peer: PeerHandle, _trusted: bool) -> BackendFuture<'_, ()> {\n        unsupported_future("Bluetooth trust management is not implemented until P4-004")\n    }''',
    '''    fn pair(&self, peer: PeerHandle) -> BackendFuture<'_, ()> {\n        let pairing = Arc::clone(&self.pairing);\n        Box::pin(async move { pair_peer(&self.connection, &pairing, &peer).await })\n    }\n\n    fn set_trusted(&self, peer: PeerHandle, trusted: bool) -> BackendFuture<'_, ()> {\n        Box::pin(async move { set_peer_trusted(&self.connection, &peer, trusted).await })\n    }''',
)

marker = 'struct BluezAdapterSubscription {'
insert = r'''
#[derive(Debug, Default)]
struct PairingControl {
    active_peer: Mutex<Option<String>>,
}

impl PairingControl {
    fn begin(self: &Arc<Self>, peer: &PeerHandle) -> Result<PairingPermit, CoreError> {
        let mut active = self.active_peer.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::Internal,
                "Bluetooth pairing authorization state is unavailable",
            )
        })?;
        if active.is_some() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "another Bluetooth pairing operation is already active",
            ));
        }
        let peer = peer.as_str().to_owned();
        *active = Some(peer.clone());
        Ok(PairingPermit {
            control: Arc::clone(self),
            peer,
        })
    }

    fn authorizes(&self, device: &OwnedObjectPath) -> bool {
        self.active_peer
            .lock()
            .map(|active| active.as_deref() == Some(device.as_str()))
            .unwrap_or(false)
    }
}

struct PairingPermit {
    control: Arc<PairingControl>,
    peer: String,
}

impl Drop for PairingPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.control.active_peer.lock()
            && active.as_deref() == Some(self.peer.as_str())
        {
            *active = None;
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
        self.require_authorized(&device)
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
        .map_err(|error| pairing_agent_registration_error("serve the BlueRoute pairing agent", error))?;

    let manager = Proxy::new(
        connection,
        BLUEZ_SERVICE,
        BLUEZ_ROOT_PATH,
        AGENT_MANAGER_INTERFACE,
    )
    .await
    .map_err(|error| pairing_agent_registration_error("create the BlueZ agent-manager proxy", error))?;
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

fn pairing_agent_registration_error(operation: &'static str, error: impl std::fmt::Display) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::CapabilityUnavailable,
        format!("failed to {operation}"),
        error.to_string(),
    )
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
        "org.bluez.Error.AuthenticationRejected" => {
            (ErrorKind::AuthenticationFailed, "Bluetooth pairing was rejected")
        }
        "org.bluez.Error.AuthenticationCanceled" => {
            (ErrorKind::AuthenticationFailed, "Bluetooth pairing was canceled")
        }
        "org.bluez.Error.AuthenticationFailed" => {
            (ErrorKind::AuthenticationFailed, "Bluetooth authentication failed")
        }
        "org.bluez.Error.AuthenticationTimeout" => {
            (ErrorKind::PairingFailed, "Bluetooth pairing timed out")
        }
        "org.bluez.Error.ConnectionAttemptFailed" => {
            (ErrorKind::PairingFailed, "Bluetooth connection attempt failed")
        }
        "org.bluez.Error.InvalidArguments" => {
            (ErrorKind::InvalidInput, "Bluetooth pairing request is invalid")
        }
        "org.bluez.Error.InProgress" => {
            (ErrorKind::InvalidState, "Bluetooth pairing is already in progress")
        }
        "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject" => {
            (ErrorKind::InvalidState, "Bluetooth peer is no longer available")
        }
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
        .map_err(trust_error)
}

fn trust_error(error: zbus::Error) -> CoreError {
    let kind = match &error {
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.DoesNotExist" | "org.freedesktop.DBus.Error.UnknownObject"
            ) => ErrorKind::InvalidState,
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.bluez.Error.NotAuthorized" | "org.freedesktop.DBus.Error.AccessDenied"
            ) => ErrorKind::AuthenticationFailed,
        _ => ErrorKind::AuthenticationFailed,
    };
    CoreError::with_diagnostic(
        kind,
        "failed to change Bluetooth trust state",
        error.to_string(),
    )
}

'''
if marker not in text:
    raise SystemExit('BluezAdapterSubscription marker not found')
text = text.replace(marker, insert + marker, 1)
text = text.replace(
    '''fn unsupported_future(message: &'static str) -> BackendFuture<'static, ()> {\n    Box::pin(async move { Err(CoreError::new(ErrorKind::CapabilityUnavailable, message)) })\n}\n\n''',
    '',
)

end_marker = '''    #[test]\n    fn unchanged_snapshot_produces_no_events() {\n        let snapshot = vec![adapter("/org/bluez/hci0", true)];\n        assert!(diff_adapter_snapshots(&snapshot, &snapshot).is_empty());\n    }\n}'''
replacement = '''    #[test]\n    fn unchanged_snapshot_produces_no_events() {\n        let snapshot = vec![adapter("/org/bluez/hci0", true)];\n        assert!(diff_adapter_snapshots(&snapshot, &snapshot).is_empty());\n    }\n\n    #[test]\n    fn pairing_agent_only_authorizes_the_active_peer() {\n        let control = Arc::new(PairingControl::default());\n        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();\n        let other = OwnedObjectPath::try_from("/org/bluez/hci0/dev_11_22_33_44_55_66").unwrap();\n        let active = OwnedObjectPath::try_from(peer.as_str()).unwrap();\n        let permit = control.begin(&peer).unwrap();\n        assert!(control.authorizes(&active));\n        assert!(!control.authorizes(&other));\n        drop(permit);\n        assert!(!control.authorizes(&active));\n    }\n\n    #[test]\n    fn pairing_control_rejects_concurrent_pairing_operations() {\n        let control = Arc::new(PairingControl::default());\n        let first = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();\n        let second = PeerHandle::new("/org/bluez/hci0/dev_11_22_33_44_55_66").unwrap();\n        let _permit = control.begin(&first).unwrap();\n        let error = match control.begin(&second) {\n            Ok(_) => panic!("concurrent pairing unexpectedly succeeded"),\n            Err(error) => error,\n        };\n        assert_eq!(error.kind(), ErrorKind::InvalidState);\n    }\n\n    #[test]\n    fn pairing_rejection_and_timeout_have_distinct_typed_errors() {\n        assert_eq!(\n            pairing_method_error("org.bluez.Error.AuthenticationRejected"),\n            (ErrorKind::AuthenticationFailed, "Bluetooth pairing was rejected")\n        );\n        assert_eq!(\n            pairing_method_error("org.bluez.Error.AuthenticationTimeout"),\n            (ErrorKind::PairingFailed, "Bluetooth pairing timed out")\n        );\n        assert_eq!(pairing_timeout_error().kind(), ErrorKind::PairingFailed);\n    }\n\n    #[test]\n    fn pairing_input_requests_are_rejected_for_no_input_no_output_agent() {\n        let control = Arc::new(PairingControl::default());\n        let peer = PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();\n        let _permit = control.begin(&peer).unwrap();\n        let agent = PairingAgent { control };\n        let path = OwnedObjectPath::try_from(peer.as_str()).unwrap();\n        assert!(agent.request_pin_code(path.clone()).is_err());\n        assert!(agent.request_passkey(path).is_err());\n    }\n}'''
if end_marker not in text:
    raise SystemExit('test end marker not found')
text = text.replace(end_marker, replacement, 1)
path.write_text(text)

example = Path('crates/blueroute-linux/examples/bluez_pair_probe.rs')
example.write_text(r'''use std::env;
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

use blueroute_linux::{BluetoothBackend, BluezBackend, DiscoveredPeer};
use futures_lite::future;

const DISCOVERY_WINDOW: Duration = Duration::from_secs(10);

fn main() -> Result<(), Box<dyn Error>> {
    let target = env::args().nth(1).ok_or_else(|| {
        io::Error::other("usage: bluez_pair_probe <exact display name or BlueZ peer path>")
    })?;

    future::block_on(async move {
        let backend = BluezBackend::connect_system().await?;
        let adapter = backend
            .adapters()
            .await?
            .into_iter()
            .find(|adapter| adapter.powered)
            .ok_or_else(|| io::Error::other("no powered Bluetooth adapter is available"))?;

        println!("adapter: {}", adapter.handle.as_str());
        println!("discovering for {} seconds...", DISCOVERY_WINDOW.as_secs());
        backend.start_discovery(adapter.handle.clone()).await?;
        thread::sleep(DISCOVERY_WINDOW);
        let peers = backend.discovered_peers(adapter.handle.clone()).await;
        let stop = backend.stop_discovery(adapter.handle.clone()).await;
        let peers = peers?;
        stop?;

        let peer = find_peer(&peers, &target).ok_or_else(|| {
            io::Error::other(format!("Bluetooth peer {target:?} was not discovered"))
        })?;
        println!(
            "target: {} name={} paired={} trusted={}",
            peer.handle.as_str(),
            peer.display_name.as_deref().unwrap_or("<unknown>"),
            peer.paired,
            peer.trusted
        );

        backend.pair(peer.handle.clone()).await?;
        backend.set_trusted(peer.handle.clone(), true).await?;

        let refreshed = backend
            .discovered_peers(adapter.handle)
            .await?
            .into_iter()
            .find(|candidate| candidate.handle == peer.handle)
            .ok_or_else(|| io::Error::other("paired peer disappeared from BlueZ"))?;
        if !refreshed.paired || !refreshed.trusted {
            return Err(io::Error::other(format!(
                "pairing did not converge: paired={} trusted={}",
                refreshed.paired, refreshed.trusted
            ))
            .into());
        }
        println!("pairing complete: paired=true trusted=true");
        Ok::<(), Box<dyn Error>>(())
    })
}

fn find_peer<'a>(peers: &'a [DiscoveredPeer], target: &str) -> Option<&'a DiscoveredPeer> {
    peers.iter().find(|peer| {
        peer.handle.as_str() == target || peer.display_name.as_deref() == Some(target)
    })
}
''')

doc = Path('docs/P4-004-PAIRING.md')
doc.write_text(r'''# P4-004 BlueZ Pairing and Trust

BlueRoute performs pairing directly through `org.bluez.Device1.Pair` and changes BlueZ trust through the `org.bluez.Device1.Trusted` property. Production code does not invoke or parse `bluetoothctl`.

## Application agent

Before an outgoing pairing operation, the Linux backend serves `org.bluez.Agent1` at `/org/blueroute/PairingAgent` and registers it with `org.bluez.AgentManager1` using the `NoInputNoOutput` capability. It is an application agent, not a requested system-wide default agent.

The agent authorizes confirmation, authorization, and service callbacks only for the exact peer whose `BluetoothBackend::pair` operation is active. Unrelated callbacks are rejected. PIN-code and passkey-input requests are rejected because a `NoInputNoOutput` BlueRoute process cannot honestly satisfy them.

Only one outgoing pairing operation may be active through a backend instance at a time. Authorization is automatically revoked when that operation finishes or fails.

## Timeout and errors

BlueRoute bounds an outgoing pairing call to 60 seconds. On timeout it attempts `Device1.CancelPairing` before returning `ErrorKind::PairingFailed`.

BlueZ authentication rejection/cancellation/failure are translated into `ErrorKind::AuthenticationFailed`; connection and timeout failures use `ErrorKind::PairingFailed`. Invalid or stale requests retain typed state/input errors and low-level D-Bus context remains diagnostic-only.

## Trust policy

Pairing and BlueZ trust are deliberately separate. `pair()` does not silently mark a peer trusted. `set_trusted(peer, true)` requires the peer to already be paired; untrusting is always allowed. Neither operation changes BlueRoute network membership.

## Hardware acceptance probe

The probe below discovers a peer by exact BlueZ object path or exact display name, invokes the Rust pairing adapter, sets trust, and verifies the refreshed `Device1` state:

```bash
cargo run -p blueroute-linux --example bluez_pair_probe --locked -- debiancb1
```

The remote test node must be powered, discoverable/pairable as appropriate, and have an authentication agent capable of completing its side of the pairing exchange. The P4-004 task remains in progress until two Linux test nodes complete this Rust-initiated flow and the evidence is recorded.
''')

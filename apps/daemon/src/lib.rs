#![doc = "BlueRoute daemon D-Bus service implementation."]

use std::fmt;

use blueroute_core::{HealthLevel, NodeCapabilities, NodeId};
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DaemonStatus, Event,
    ProtocolCodecError, Response, decode_command, encode_event, encode_response,
};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, interface};

/// Versioned local D-Bus interface implementation.
#[derive(Clone, Debug)]
pub struct DaemonService {
    status: DaemonStatus,
}

impl DaemonService {
    pub fn new(local_node: NodeId, health: HealthLevel, capabilities: NodeCapabilities) -> Self {
        Self {
            status: DaemonStatus {
                api_version: ApiVersion::CURRENT,
                local_node: Some(local_node),
                current_network: None,
                health,
                capabilities,
            },
        }
    }

    pub fn from_status(status: DaemonStatus) -> Self {
        Self { status }
    }

    pub fn status_snapshot(&self) -> &DaemonStatus {
        &self.status
    }

    fn encode_response(response: &Response) -> fdo::Result<String> {
        encode_response(response).map_err(|_| {
            fdo::Error::Failed("BlueRoute failed to encode its local API response".into())
        })
    }
}

#[interface(name = "org.blueroute.Service1")]
impl DaemonService {
    fn version(&self) -> (u16, u16) {
        (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor)
    }

    fn status(&self) -> fdo::Result<String> {
        Self::encode_response(&Response::Status(self.status.clone()))
    }

    fn capabilities(&self) -> fdo::Result<String> {
        Self::encode_response(&Response::Capabilities(self.status.capabilities.clone()))
    }

    fn request(&self, payload: &str) -> fdo::Result<String> {
        let command = decode_command(payload)
            .map_err(|_| fdo::Error::InvalidArgs("malformed BlueRoute command payload".into()))?;
        match command {
            Command::GetStatus => self.status(),
            Command::GetCapabilities => self.capabilities(),
            _ => Err(fdo::Error::NotSupported(
                "command is not implemented by the P5-004 daemon skeleton".into(),
            )),
        }
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;
}

/// Emit one already-typed protocol event through the daemon's D-Bus signal.
pub async fn emit_event(connection: &Connection, event: &Event) -> Result<(), DaemonServiceError> {
    let payload = encode_event(event).map_err(DaemonServiceError::Protocol)?;
    let interface = connection
        .object_server()
        .interface::<_, DaemonService>(DBUS_OBJECT_PATH)
        .await
        .map_err(DaemonServiceError::Bus)?;
    DaemonService::event(interface.signal_emitter(), &payload)
        .await
        .map_err(DaemonServiceError::Bus)
}

#[derive(Debug)]
pub enum DaemonServiceError {
    Protocol(ProtocolCodecError),
    Bus(zbus::Error),
}

impl fmt::Display for DaemonServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => write!(formatter, "protocol encoding failed: {error}"),
            Self::Bus(error) => write!(formatter, "D-Bus operation failed: {error}"),
        }
    }
}

impl std::error::Error for DaemonServiceError {}

/// Compile-time guard that keeps the implementation interface synchronized with protocol constants.
pub fn dbus_interface_name() -> &'static str {
    DBUS_INTERFACE_NAME
}

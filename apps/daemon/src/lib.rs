#![doc = "BlueRoute daemon D-Bus service implementation."]

mod authorization;
mod single_star;

use std::fmt;
use std::sync::{Arc, Mutex};

pub use authorization::{
    CommandAuthorization, INTERNET_SHARING_ACTION_ID, MODIFY_ACTION_ID, command_authorization,
    command_operation,
};
use blueroute_core::{CoreError, ErrorKind, HealthLevel, NetworkId, NodeCapabilities, NodeId};
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DaemonStatus, Event,
    ProtocolCodecError, Response, decode_command, encode_event, encode_response,
};
pub use single_star::{
    LinuxStarHostRuntime, NetworkIdGenerator, NetworkOperationFuture, NetworkOperations,
    SingleStarNetworkOperations, StarHostRuntime, SystemNetworkIdGenerator, current_network,
};
use zbus::fdo;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, interface};

/// Versioned local D-Bus interface implementation.
#[derive(Clone)]
pub struct DaemonService {
    status: Arc<Mutex<DaemonStatus>>,
    network_operations: Option<Arc<dyn NetworkOperations>>,
}

impl DaemonService {
    pub fn new(local_node: NodeId, health: HealthLevel, capabilities: NodeCapabilities) -> Self {
        Self::from_status(DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(local_node),
            current_network: None,
            health,
            capabilities,
        })
    }

    pub fn from_status(status: DaemonStatus) -> Self {
        Self {
            status: Arc::new(Mutex::new(status)),
            network_operations: None,
        }
    }

    pub fn with_network_operations(
        status: DaemonStatus,
        network_operations: Arc<dyn NetworkOperations>,
    ) -> Self {
        Self {
            status: Arc::new(Mutex::new(status)),
            network_operations: Some(network_operations),
        }
    }

    pub fn status_snapshot(&self) -> Result<DaemonStatus, CoreError> {
        self.status
            .lock()
            .map(|status| status.clone())
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "BlueRoute daemon status lock was poisoned",
                    error.to_string(),
                )
            })
    }

    fn encode_response(response: &Response) -> fdo::Result<String> {
        encode_response(response).map_err(|_| {
            fdo::Error::Failed("BlueRoute failed to encode its local API response".into())
        })
    }

    fn network_operations(&self) -> fdo::Result<&Arc<dyn NetworkOperations>> {
        self.network_operations.as_ref().ok_or_else(|| {
            fdo::Error::NotSupported(
                "network lifecycle operations are unavailable in this daemon instance".into(),
            )
        })
    }

    fn update_current_network(&self, network: Option<NetworkId>) -> fdo::Result<()> {
        let mut status = self
            .status
            .lock()
            .map_err(|_| fdo::Error::Failed("BlueRoute daemon status is unavailable".into()))?;
        status.current_network = network;
        Ok(())
    }
}

#[interface(name = "org.blueroute.Service1")]
impl DaemonService {
    fn version(&self) -> (u16, u16) {
        (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor)
    }

    fn status(&self) -> fdo::Result<String> {
        let status = self.status_snapshot().map_err(core_error_to_dbus)?;
        Self::encode_response(&Response::Status(status))
    }

    fn capabilities(&self) -> fdo::Result<String> {
        let capabilities = self
            .status_snapshot()
            .map_err(core_error_to_dbus)?
            .capabilities;
        Self::encode_response(&Response::Capabilities(capabilities))
    }

    async fn request(
        &self,
        payload: &str,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<String> {
        let command = decode_command(payload)
            .map_err(|_| fdo::Error::InvalidArgs("malformed BlueRoute command payload".into()))?;

        authorization::authorize_command(connection, &header, &command).await?;

        match command {
            Command::GetStatus => self.status(),
            Command::GetCapabilities => self.capabilities(),
            Command::ListNetworks => {
                let networks = self
                    .network_operations()?
                    .list_networks()
                    .await
                    .map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Networks(networks))
            }
            Command::CreateNetwork { name } => {
                let network = self
                    .network_operations()?
                    .create_network(name)
                    .await
                    .map_err(core_error_to_dbus)?;
                self.update_current_network(Some(network))?;
                Self::encode_response(&Response::Ack)
            }
            Command::StartDiscovery => {
                self.network_operations()?
                    .start_discovery()
                    .await
                    .map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Ack)
            }
            Command::StopDiscovery => {
                self.network_operations()?
                    .stop_discovery()
                    .await
                    .map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Ack)
            }
            _ => Err(fdo::Error::NotSupported(
                "command is not implemented by the current daemon".into(),
            )),
        }
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;
}

fn core_error_to_dbus(error: CoreError) -> fdo::Error {
    match error.kind() {
        ErrorKind::InvalidInput => fdo::Error::InvalidArgs(error.message().to_owned()),
        ErrorKind::CapabilityUnavailable
        | ErrorKind::MissingAdapter
        | ErrorKind::AdapterDisabled
        | ErrorKind::UnsupportedRuntime => fdo::Error::NotSupported(error.message().to_owned()),
        _ => {
            let message = match error.diagnostic() {
                Some(diagnostic) => format!("{} ({diagnostic})", error.message()),
                None => error.message().to_owned(),
            };
            fdo::Error::Failed(message)
        }
    }
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

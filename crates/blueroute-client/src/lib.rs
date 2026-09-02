#![doc = "Reusable client for the BlueRoute daemon API."]

use std::fmt;
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_core::NodeCapabilities;
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus,
    Event, ProtocolCodecError, Response, decode_event, decode_response, encode_command,
};
use futures_lite::StreamExt;
use zbus::proxy::SignalStream;
use zbus::{Connection, Proxy};

const RECONNECT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Reusable version-gated client for the local BlueRoute daemon.
#[derive(Clone)]
pub struct BlueRouteClient {
    connection: Connection,
    server_version: ApiVersion,
}

impl BlueRouteClient {
    /// Connect to the production system bus and reject an incompatible daemon before returning.
    pub async fn connect() -> Result<Self, ClientError> {
        let connection = Connection::system().await.map_err(ClientError::Bus)?;
        Self::from_connection(connection).await
    }

    /// Build a client over an established bus connection.
    ///
    /// This is useful for deterministic tests and callers that already own a zbus connection.
    pub async fn from_connection(connection: Connection) -> Result<Self, ClientError> {
        let server_version = negotiate_version(&connection).await?;
        Ok(Self {
            connection,
            server_version,
        })
    }

    pub const fn server_version(&self) -> ApiVersion {
        self.server_version
    }

    /// Wait for a compatible daemon to become available again after a restart.
    ///
    /// Only the read-only `Version` method is retried. Normal commands are never replayed
    /// automatically because retrying a mutation after a lost reply could duplicate side effects.
    pub async fn reconnect(&mut self, timeout: Duration) -> Result<(), ClientError> {
        let deadline = Instant::now() + timeout;
        loop {
            match negotiate_version(&self.connection).await {
                Ok(server_version) => {
                    self.server_version = server_version;
                    return Ok(());
                }
                Err(error @ ClientError::IncompatibleVersion { .. }) => return Err(error),
                Err(ClientError::Bus(_)) if Instant::now() < deadline => {
                    Timer::after(RECONNECT_POLL_INTERVAL).await;
                }
                Err(ClientError::Bus(error)) => {
                    return Err(ClientError::ReconnectTimeout {
                        timeout,
                        last_error: error.to_string(),
                    });
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Send one typed protocol command.
    ///
    /// Version compatibility is checked immediately before every normal command. This makes a
    /// daemon restart to an incompatible API fail closed before the command can be delivered.
    pub async fn request(&self, command: &Command) -> Result<Response, ClientError> {
        negotiate_version(&self.connection).await?;
        let payload = encode_command(command).map_err(ClientError::Protocol)?;
        let proxy = service_proxy(&self.connection).await?;
        let response_payload: String = proxy
            .call("Request", &(payload.as_str(),))
            .await
            .map_err(ClientError::Bus)?;
        decode_response(&response_payload).map_err(ClientError::Protocol)
    }

    pub async fn status(&self) -> Result<DaemonStatus, ClientError> {
        match self.request(&Command::GetStatus).await? {
            Response::Status(status) => Ok(status),
            other => Err(ClientError::UnexpectedResponse {
                expected: "status",
                actual: response_kind(&other),
            }),
        }
    }

    pub async fn capabilities(&self) -> Result<NodeCapabilities, ClientError> {
        match self.request(&Command::GetCapabilities).await? {
            Response::Capabilities(capabilities) => Ok(capabilities),
            other => Err(ClientError::UnexpectedResponse {
                expected: "capabilities",
                actual: response_kind(&other),
            }),
        }
    }

    /// Subscribe to typed daemon events after verifying the current daemon API version.
    pub async fn events(&self) -> Result<EventSubscription, ClientError> {
        negotiate_version(&self.connection).await?;
        let proxy = service_proxy(&self.connection).await?;
        let stream = proxy
            .receive_signal("Event")
            .await
            .map_err(ClientError::Bus)?;
        Ok(EventSubscription {
            connection: self.connection.clone(),
            stream,
        })
    }
}

/// Event stream that rechecks daemon compatibility before accepting every event payload.
pub struct EventSubscription {
    connection: Connection,
    stream: SignalStream<'static>,
}

impl EventSubscription {
    pub async fn next_event(&mut self) -> Result<Event, ClientError> {
        let message = self
            .stream
            .next()
            .await
            .ok_or(ClientError::EventStreamClosed)?;
        // Signal streams can follow a well-known D-Bus name across owner changes. Re-negotiate here
        // so an event from a replacement daemon cannot bypass the same version gate as commands.
        negotiate_version(&self.connection).await?;
        let payload: String = message
            .body()
            .deserialize()
            .map_err(|error| ClientError::InvalidTransportPayload(error.to_string()))?;
        decode_event(&payload).map_err(ClientError::Protocol)
    }
}

async fn negotiate_version(connection: &Connection) -> Result<ApiVersion, ClientError> {
    let proxy = service_proxy(connection).await?;
    let (major, minor): (u16, u16) = proxy.call("Version", &()).await.map_err(ClientError::Bus)?;
    let server = ApiVersion::new(major, minor);
    if !ApiVersion::CURRENT.is_compatible_with_server(server) {
        return Err(ClientError::IncompatibleVersion {
            client: ApiVersion::CURRENT,
            server,
        });
    }
    Ok(server)
}

async fn service_proxy(connection: &Connection) -> Result<Proxy<'static>, ClientError> {
    Proxy::new_owned(
        connection.clone(),
        DBUS_SERVICE_NAME,
        DBUS_OBJECT_PATH,
        DBUS_INTERFACE_NAME,
    )
    .await
    .map_err(ClientError::Bus)
}

fn response_kind(response: &Response) -> &'static str {
    match response {
        Response::Ack => "ack",
        Response::Status(_) => "status",
        Response::Capabilities(_) => "capabilities",
        Response::Networks(_) => "networks",
        Response::Nodes(_) => "nodes",
        Response::Node(_) => "node",
        Response::Diagnostics(_) => "diagnostics",
    }
}

#[derive(Debug)]
pub enum ClientError {
    Bus(zbus::Error),
    Protocol(ProtocolCodecError),
    IncompatibleVersion {
        client: ApiVersion,
        server: ApiVersion,
    },
    UnexpectedResponse {
        expected: &'static str,
        actual: &'static str,
    },
    InvalidTransportPayload(String),
    EventStreamClosed,
    ReconnectTimeout {
        timeout: Duration,
        last_error: String,
    },
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(error) => write!(formatter, "D-Bus operation failed: {error}"),
            Self::Protocol(error) => write!(formatter, "protocol payload failed: {error}"),
            Self::IncompatibleVersion { client, server } => write!(
                formatter,
                "incompatible BlueRoute API: client={client} server={server}"
            ),
            Self::UnexpectedResponse { expected, actual } => write!(
                formatter,
                "daemon returned response type {actual} where {expected} was required"
            ),
            Self::InvalidTransportPayload(error) => {
                write!(
                    formatter,
                    "daemon event carried an invalid D-Bus payload: {error}"
                )
            }
            Self::EventStreamClosed => formatter.write_str("daemon event stream closed"),
            Self::ReconnectTimeout {
                timeout,
                last_error,
            } => write!(
                formatter,
                "compatible daemon did not return within {timeout:?}: {last_error}"
            ),
        }
    }
}

impl std::error::Error for ClientError {}

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";

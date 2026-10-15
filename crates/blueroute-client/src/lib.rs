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

/// Stable error categories shared by the CLI and other daemon clients.
/// Only exact D-Bus protocol error names classify remote failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientErrorKind {
    Unavailable,
    Authorization,
    Unsupported,
    Operational,
}

fn remote_error_kind(name: &str) -> ClientErrorKind {
    match name {
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" => {
            ClientErrorKind::Authorization
        }
        "org.freedesktop.DBus.Error.NotSupported" => ClientErrorKind::Unsupported,
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.NoReply"
        | "org.freedesktop.DBus.Error.NoServer" => ClientErrorKind::Unavailable,
        _ => ClientErrorKind::Operational,
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

impl ClientError {
    /// Classify a specific typed client failure; do not interpret
    /// peer-controlled textual diagnostics as authorization decisions.
    pub fn kind(&self) -> ClientErrorKind {
        match self {
            Self::IncompatibleVersion { .. } | Self::ReconnectTimeout { .. } => {
                ClientErrorKind::Unavailable
            }
            Self::Bus(zbus::Error::MethodError(name, _, _)) => {
                remote_error_kind(name.as_str())
            }
            Self::Bus(zbus::Error::FDO(error)) => match error.as_ref() {
                zbus::fdo::Error::AccessDenied(_)
                | zbus::fdo::Error::AuthFailed(_)
                | zbus::fdo::Error::InteractiveAuthorizationRequired(_) => {
                    ClientErrorKind::Authorization
                }
                zbus::fdo::Error::NotSupported(_) => ClientErrorKind::Unsupported,
                zbus::fdo::Error::ServiceUnknown(_)
                | zbus::fdo::Error::NameHasNoOwner(_)
                | zbus::fdo::Error::NoReply(_)
                | zbus::fdo::Error::NoServer(_) => ClientErrorKind::Unavailable,
                _ => ClientErrorKind::Operational,
            },
            Self::Bus(
                zbus::Error::InputOutput(_)
                | zbus::Error::Address(_)
                | zbus::Error::Handshake(_),
            ) => ClientErrorKind::Unavailable,
            _ => ClientErrorKind::Operational,
        }
    }
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

#[cfg(test)]
mod error_class_tests {
    use super::*;

    #[test]
    fn error_names_are_exact_and_never_classify_untrusted_message_text() {
        assert_eq!(
            remote_error_kind("org.freedesktop.DBus.Error.AccessDenied"),
            ClientErrorKind::Authorization
        );
        assert_eq!(
            remote_error_kind("org.freedesktop.DBus.Error.NotSupported"),
            ClientErrorKind::Unsupported
        );
        assert_eq!(
            remote_error_kind("org.freedesktop.DBus.Error.ServiceUnknown"),
            ClientErrorKind::Unavailable
        );
        for suspicious in [
            "AccessDenied",
            "org.freedesktop.DBus.Error.AccessDenied.extra",
            "Error: AccessDenied",
            "org.freedesktop.DBus.Error.Failed",
        ] {
            assert_eq!(
                remote_error_kind(suspicious),
                ClientErrorKind::Operational
            );
        }
    }

    #[test]
    fn version_mismatch_is_unavailable_and_unrelated_errors_remain_operational() {
        assert_eq!(
            ClientError::IncompatibleVersion {
                client: ApiVersion::new(1, 0),
                server: ApiVersion::new(2, 0),
            }
            .kind(),
            ClientErrorKind::Unavailable
        );
        assert_eq!(
            ClientError::EventStreamClosed.kind(),
            ClientErrorKind::Operational
        );
        assert_eq!(
            ClientError::Bus(zbus::Error::FDO(Box::new(
                zbus::fdo::Error::AccessDenied("denied".into())
            )))
            .kind(),
            ClientErrorKind::Authorization
        );
    }
}

/// The human-readable project name.
pub const PROJECT_NAME: &str = "BlueRoute";

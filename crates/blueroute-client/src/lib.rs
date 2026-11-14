#![doc = "Reusable client for the BlueRoute daemon API."]

mod watch;
pub use watch::{WatchSignal, reconnect_delay, watch};

use std::fmt;
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_core::NodeCapabilities;
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus,
    Event, ProtocolCodecError, Response, STATE_CHANGED_SIGNAL, decode_event, decode_response,
    encode_command,
};
use futures_lite::StreamExt;
use zbus::fdo::{DBusProxy, NameOwnerChangedStream};
use zbus::names::{BusName, OwnedUniqueName};
use zbus::proxy::SignalStream;
use zbus::{Connection, Proxy};

const RECONNECT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Reusable version-gated client for the local BlueRoute daemon.
///
/// Every operation resolves the daemon's well-known name to its unique bus name once, checks the
/// version there, and talks only to that unique name (CR-F8, OWN-1..OWN-4). A replacement owner
/// that takes the name mid-operation is never reached; an owner that disappears makes the
/// operation fail as unavailable. Nothing is ever retried automatically.
#[derive(Clone)]
pub struct BlueRouteClient {
    connection: Connection,
    /// The version of the owner most recently validated. Informational only: every operation
    /// validates its own owner again.
    server_version: ApiVersion,
}

/// A daemon owner whose compatible version was checked at its unique bus name.
struct ValidatedOwner {
    name: OwnedUniqueName,
    version: ApiVersion,
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
        let owner = validated_owner(&connection).await?;
        Ok(Self {
            connection,
            server_version: owner.version,
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
            match validated_owner(&self.connection).await {
                Ok(owner) => {
                    self.server_version = owner.version;
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
    /// The command goes to exactly the owner whose compatible version was checked immediately
    /// before, addressed by its unique bus name. A daemon replaced in between never receives it,
    /// and a reply lost in transit is reported, never answered by resending the command.
    pub async fn request(&self, command: &Command) -> Result<Response, ClientError> {
        let owner = validated_owner(&self.connection).await?;
        let payload = encode_command(command).map_err(ClientError::Protocol)?;
        let proxy = owner_proxy(&self.connection, &owner.name).await?;
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

    /// Subscribe to typed daemon events from the owner whose compatible version was just checked.
    /// Signals are matched on that owner's unique bus name, so a replacement owner's events never
    /// arrive on this subscription. The subscription also watches the name itself and ends with
    /// [`ClientError::OwnerLost`] when that owner gives it up or disappears, even if the daemon
    /// never emits another event (CR-F2, WATCH-1).
    pub async fn events(&self) -> Result<EventSubscription, ClientError> {
        self.subscribe(false).await
    }

    /// Like [`Self::events`], but also delivers the daemon's `StateChanged` invalidations
    /// (API 1.7) as [`Change::State`] through [`EventSubscription::next_change`]. This is what a
    /// live view needs: any change it could display, not only typed events (CR-F1).
    pub async fn changes(&self) -> Result<EventSubscription, ClientError> {
        self.subscribe(true).await
    }

    async fn subscribe(&self, with_state: bool) -> Result<EventSubscription, ClientError> {
        // Watch the name before resolving its owner, so no ownership change can fall between
        // the two steps unobserved.
        let owner_changes = DBusProxy::new(&self.connection)
            .await
            .map_err(ClientError::Bus)?
            .receive_name_owner_changed_with_args(&[(0, DBUS_SERVICE_NAME)])
            .await
            .map_err(ClientError::Bus)?;
        let owner = validated_owner(&self.connection).await?;
        let proxy = owner_proxy(&self.connection, &owner.name).await?;
        let stream = proxy
            .receive_signal("Event")
            .await
            .map_err(ClientError::Bus)?;
        let state = if with_state {
            Some(
                proxy
                    .receive_signal(STATE_CHANGED_SIGNAL)
                    .await
                    .map_err(ClientError::Bus)?,
            )
        } else {
            None
        };
        Ok(EventSubscription {
            connection: self.connection.clone(),
            owner: owner.name,
            stream,
            state,
            owner_changes,
            retired: false,
        })
    }
}

/// One thing a [`BlueRouteClient::changes`] subscription reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A typed daemon event.
    Event(Event),
    /// Something observable changed; reread. The revision only orders signals of one daemon
    /// process and is not a state version (see `blueroute_protocol::STATE_CHANGED_SIGNAL`).
    State { revision: u64 },
}

/// Event stream that rechecks daemon compatibility before accepting every event payload.
pub struct EventSubscription {
    connection: Connection,
    /// The unique bus name the subscription is bound to.
    owner: OwnedUniqueName,
    stream: SignalStream<'static>,
    /// `StateChanged` invalidations, for a [`BlueRouteClient::changes`] subscription.
    state: Option<SignalStream<'static>>,
    /// Ownership changes of the daemon's well-known name only.
    owner_changes: NameOwnerChangedStream,
    /// Set once the bound owner was seen to go; nothing is delivered afterwards (FU-02).
    retired: bool,
}

/// Resolves when an ownership change retires `owner`: the name moved to another owner or was
/// released, or the bus connection closed. A change that leaves the name with `owner` is only
/// the queued notice of it taking the name, and is skipped.
async fn owner_retired(changes: &mut NameOwnerChangedStream, owner: &OwnedUniqueName) {
    loop {
        match changes.next().await {
            Some(change) => {
                let still_ours = change
                    .args()
                    .ok()
                    .and_then(|args| args.new_owner().as_ref().map(|new| new == owner))
                    == Some(true);
                if !still_ours {
                    return;
                }
            }
            None => return,
        }
    }
}

/// What the next thing to happen on a subscription was.
enum Next {
    Event(Option<zbus::Message>),
    State(Option<zbus::Message>),
    Retired,
}

impl EventSubscription {
    /// The next typed event. On a [`BlueRouteClient::changes`] subscription, invalidations are
    /// skipped; use [`Self::next_change`] there.
    pub async fn next_event(&mut self) -> Result<Event, ClientError> {
        loop {
            if let Change::Event(event) = self.next_change().await? {
                return Ok(event);
            }
        }
    }

    /// The next event or invalidation. Ends with [`ClientError::OwnerLost`] when the bound owner
    /// goes away, and every item is accepted only after that same owner's version is rechecked.
    ///
    /// Delivery decision (FU-02, OWNER-1..3): the owner is watched the whole time, including
    /// while an item's version is checked. An item is delivered only if, after that check, the
    /// bound owner still holds the name; a retirement seen at any point ends the subscription for
    /// good, and the pending check (and its late reply) is dropped.
    pub async fn next_change(&mut self) -> Result<Change, ClientError> {
        if self.retired {
            return Err(ClientError::OwnerLost);
        }
        let Self {
            connection,
            owner,
            stream,
            state,
            owner_changes,
            retired,
        } = self;
        let signals = futures_lite::future::or(async { Next::Event(stream.next().await) }, async {
            match state {
                Some(state) => Next::State(state.next().await),
                None => futures_lite::future::pending().await,
            }
        });
        let next = futures_lite::future::or(signals, async {
            owner_retired(owner_changes, owner).await;
            Next::Retired
        })
        .await;
        let is_state = matches!(next, Next::State(_));
        let message = match next {
            Next::Event(Some(message)) | Next::State(Some(message)) => message,
            Next::Event(None) | Next::State(None) => return Err(ClientError::EventStreamClosed),
            Next::Retired => {
                *retired = true;
                return Err(ClientError::OwnerLost);
            }
        };

        // The streams only carry this owner's signals; recheck that same owner's version, while
        // still watching for its retirement, so nothing is accepted on another owner's check and
        // a held reply cannot delay the loss.
        let checked = futures_lite::future::or(
            async { Some(negotiate_version_at(connection, owner).await) },
            async {
                owner_retired(owner_changes, owner).await;
                None
            },
        )
        .await;
        match checked {
            None => {
                *retired = true;
                return Err(ClientError::OwnerLost);
            }
            Some(Err(error)) => return Err(error),
            Some(Ok(_)) => {}
        }
        // The decision point: the checked owner must still be the current one.
        match current_owner(connection).await {
            Ok(current) if current == *owner => {}
            Ok(_) => {
                *retired = true;
                return Err(ClientError::OwnerLost);
            }
            // Only "nobody owns the name" retires. A transient failure of the check itself (a bus
            // timeout, for instance) is reported without ending a healthy subscription.
            Err(ClientError::Bus(zbus::Error::FDO(error)))
                if matches!(*error, zbus::fdo::Error::NameHasNoOwner(_)) =>
            {
                *retired = true;
                return Err(ClientError::OwnerLost);
            }
            Err(error) => return Err(error),
        }

        if is_state {
            let revision: u64 = message
                .body()
                .deserialize()
                .map_err(|error| ClientError::InvalidTransportPayload(error.to_string()))?;
            Ok(Change::State { revision })
        } else {
            let payload: String = message
                .body()
                .deserialize()
                .map_err(|error| ClientError::InvalidTransportPayload(error.to_string()))?;
            decode_event(&payload)
                .map(Change::Event)
                .map_err(ClientError::Protocol)
        }
    }
}

/// Resolve the well-known name to its current unique owner and check that owner's version there.
async fn validated_owner(connection: &Connection) -> Result<ValidatedOwner, ClientError> {
    let name = current_owner(connection).await?;
    let version = negotiate_version_at(connection, &name).await?;
    Ok(ValidatedOwner { name, version })
}

/// The unique bus name currently owning the daemon's well-known name. No owner is
/// `NameHasNoOwner`, which classifies as unavailable.
async fn current_owner(connection: &Connection) -> Result<OwnedUniqueName, ClientError> {
    let bus = DBusProxy::new(connection).await.map_err(ClientError::Bus)?;
    let name = BusName::try_from(DBUS_SERVICE_NAME)
        .map_err(|error| ClientError::Bus(zbus::Error::Names(error)))?;
    bus.get_name_owner(name)
        .await
        .map_err(|error| ClientError::Bus(zbus::Error::FDO(Box::new(error))))
}

async fn negotiate_version_at(
    connection: &Connection,
    owner: &OwnedUniqueName,
) -> Result<ApiVersion, ClientError> {
    let proxy = owner_proxy(connection, owner).await?;
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

/// A proxy addressed to one unique owner, never to the well-known name.
async fn owner_proxy(
    connection: &Connection,
    owner: &OwnedUniqueName,
) -> Result<Proxy<'static>, ClientError> {
    Proxy::new_owned(
        connection.clone(),
        owner.to_string(),
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
        Response::ClientLink(_) => "client_link",
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
    /// The network's private IPv4 range overlaps one already in use on this computer
    /// (`blueroute_protocol::ADDRESS_CONFLICT_ERROR`, API 1.6).
    AddressConflict,
}

fn remote_error_kind(name: &str) -> ClientErrorKind {
    match name {
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" => {
            ClientErrorKind::Authorization
        }
        "org.freedesktop.DBus.Error.NotSupported" => ClientErrorKind::Unsupported,
        blueroute_protocol::ADDRESS_CONFLICT_ERROR => ClientErrorKind::AddressConflict,
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
    /// The daemon owner this subscription was bound to gave up its name or disappeared.
    OwnerLost,
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
            Self::IncompatibleVersion { .. } | Self::ReconnectTimeout { .. } | Self::OwnerLost => {
                ClientErrorKind::Unavailable
            }
            Self::Bus(zbus::Error::MethodError(name, _, _)) => remote_error_kind(name.as_str()),
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
                zbus::Error::InputOutput(_) | zbus::Error::Address(_) | zbus::Error::Handshake(_),
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
            Self::OwnerLost => formatter.write_str("the daemon went away or was replaced"),
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
        assert_eq!(
            remote_error_kind("org.blueroute.Error.AddressConflict"),
            ClientErrorKind::AddressConflict
        );
        for suspicious in [
            "AccessDenied",
            "org.freedesktop.DBus.Error.AccessDenied.extra",
            "Error: AccessDenied",
            "org.freedesktop.DBus.Error.Failed",
            "AddressConflict",
            "org.blueroute.Error.AddressConflict.extra",
            "org.blueroute.Error.Other",
        ] {
            assert_eq!(remote_error_kind(suspicious), ClientErrorKind::Operational);
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
            ClientError::Bus(zbus::Error::FDO(Box::new(zbus::fdo::Error::AccessDenied(
                "denied".into()
            ))))
            .kind(),
            ClientErrorKind::Authorization
        );
    }
}

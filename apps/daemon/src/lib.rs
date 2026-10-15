#![doc = "BlueRoute daemon D-Bus service implementation."]

// Prepared for the future authenticated transport runtime. Keep this
// fail-closed socket owner compiled and tested without treating it as wired.
#[allow(dead_code)]
mod attempt_socket;
mod authorization;
mod control_tls;
mod join_network;
mod peer_pin_store;
mod peer_trust;
mod pinned_ingress;
mod single_star;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use authorization::{
    CommandAuthorization, INTERNET_SHARING_ACTION_ID, MODIFY_ACTION_ID, command_authorization,
    command_operation,
};
use blueroute_core::{
    CoreError, DisplayName, ErrorKind, HealthLevel, NetworkId, NodeCapabilities, NodeId,
    Reachability,
};
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DaemonStatus, DiagnosticSnapshot,
    Event, NodeSummary, ProtocolCodecError, Response, decode_command, encode_event,
    encode_response,
};
pub use control_tls::{
    ActiveControlSession, AuthenticatedControlPeer, ControlSessionError, ControlTlsError,
    ControlTlsIdentityStore, ControlTlsStream, LocalControlTlsIdentity,
    PinnedControlTlsCertificate, PinnedControlTlsPeer, VerifiedControlTlsPeer,
    accept_pinned_control_tls, connect_pinned_control_tls, exchange_control_tls_certificate,
    export_control_session_id, verify_control_tls_peer,
};
pub use join_network::{
    JoinControlSession, JoinIpLease, JoinNetworkFuture, JoinNetworkOperations, JoinPanLink,
    JoinRuntime, LinuxJoinRuntime, TransactionalJoinNetworkOperations,
};
pub use peer_pin_store::PeerPinStore;
pub use peer_trust::{
    DurablePeerTrustOperations, PeerTrustFuture, PeerTrustOperations, pair_and_trust_bluetooth_peer,
};
pub use pinned_ingress::{PinnedIngressError, PinnedSessionIngress, VerifiedTlsIdentity};
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
    peer_trust_operations: Option<Arc<dyn PeerTrustOperations>>,
    join_network_operations: Option<Arc<dyn JoinNetworkOperations>>,
    mutation_in_progress: Arc<AtomicBool>,
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
            peer_trust_operations: None,
            join_network_operations: None,
            mutation_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_network_operations(
        status: DaemonStatus,
        network_operations: Arc<dyn NetworkOperations>,
    ) -> Self {
        Self {
            status: Arc::new(Mutex::new(status)),
            network_operations: Some(network_operations),
            peer_trust_operations: None,
            join_network_operations: None,
            mutation_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_operations(
        status: DaemonStatus,
        network_operations: Arc<dyn NetworkOperations>,
        peer_trust_operations: Arc<dyn PeerTrustOperations>,
    ) -> Self {
        Self {
            status: Arc::new(Mutex::new(status)),
            network_operations: Some(network_operations),
            peer_trust_operations: Some(peer_trust_operations),
            join_network_operations: None,
            mutation_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_join_network_operations(
        mut self,
        join_network_operations: Arc<dyn JoinNetworkOperations>,
    ) -> Self {
        self.join_network_operations = Some(join_network_operations);
        self
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

    fn peer_trust_operations(&self) -> fdo::Result<&Arc<dyn PeerTrustOperations>> {
        self.peer_trust_operations.as_ref().ok_or_else(|| {
            fdo::Error::NotSupported(
                "peer trust operations are unavailable in this daemon instance".into(),
            )
        })
    }

    fn join_network_operations(&self) -> fdo::Result<&Arc<dyn JoinNetworkOperations>> {
        self.join_network_operations.as_ref().ok_or_else(|| {
            fdo::Error::NotSupported(
                "join-network operations are unavailable in this daemon instance".into(),
            )
        })
    }

    fn ensure_not_local_node(&self, node: NodeId) -> fdo::Result<()> {
        if self
            .status_snapshot()
            .map_err(core_error_to_dbus)?
            .local_node
            == Some(node)
        {
            Err(fdo::Error::InvalidArgs(
                "the local BlueRoute node cannot be approved as its own peer".into(),
            ))
        } else {
            Ok(())
        }
    }

    /// The daemon currently has no authenticated remote-node observation
    /// provider. Report only its own configured NodeId with Local reachability,
    /// never infer remote identity or reachability from untrusted advertisements
    /// or a durable trust record. The label is explicitly generic until P10
    /// device naming and verified P7 peer inventory are wired to this query.
    fn local_node_summary(&self) -> fdo::Result<Option<NodeSummary>> {
        let Some(node) = self
            .status_snapshot()
            .map_err(core_error_to_dbus)?
            .local_node
        else {
            return Ok(None);
        };
        Ok(Some(NodeSummary {
            id: node,
            name: DisplayName::new("Local node").map_err(core_error_to_dbus)?,
            reachability: Reachability::Local,
        }))
    }

    /// Read-only diagnostics never invent members from approvals, Bluetooth
    /// discovery or unverified peer-controlled Hello fields.
    pub fn diagnostics_snapshot(&self) -> Result<DiagnosticSnapshot, CoreError> {
        let status = self.status_snapshot()?;
        Ok(DiagnosticSnapshot {
            api_version: status.api_version,
            health: status.health,
            current_network: status.current_network,
            visible_nodes: u32::from(status.local_node.is_some()),
            capabilities: status.capabilities,
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
        let _mutation = matches!(
            authorization::command_authorization(&command),
            CommandAuthorization::PolicyKit(_)
        )
        .then(|| MutationGuard::claim(&self.mutation_in_progress))
        .transpose()?;

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
            Command::GetDiagnostics => {
                let snapshot = self.diagnostics_snapshot().map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Diagnostics(snapshot))
            }
            Command::ListNodes => {
                let nodes: Vec<_> = self.local_node_summary()?.into_iter().collect();
                Self::encode_response(&Response::Nodes(nodes))
            }
            Command::GetNode { node } => {
                let local = self.local_node_summary()?;
                let matching = local.filter(|entry| entry.id == node);
                Self::encode_response(&Response::Node(matching))
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
            Command::JoinNetwork { network } => {
                self.join_network_operations()?
                    .join_network(network)
                    .await
                    .map_err(core_error_to_dbus)?;
                self.update_current_network(Some(network))?;
                Self::encode_response(&Response::Ack)
            }
            Command::TrustPeer { node } => {
                self.ensure_not_local_node(node)?;
                self.peer_trust_operations()?
                    .trust_peer(node)
                    .await
                    .map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Ack)
            }
            Command::ForgetPeer { node } => {
                self.ensure_not_local_node(node)?;
                self.peer_trust_operations()?
                    .forget_peer(node)
                    .await
                    .map_err(core_error_to_dbus)?;
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
        ErrorKind::AuthenticationFailed => fdo::Error::AccessDenied(error.message().to_owned()),
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

struct MutationGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> MutationGuard<'a> {
    fn claim(flag: &'a AtomicBool) -> fdo::Result<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                fdo::Error::Failed(
                    "another BlueRoute mutation is already in progress; retry the request".into(),
                )
            })?;
        Ok(Self { flag })
    }
}

impl Drop for MutationGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
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

#[cfg(test)]
mod local_node_tests {
    use super::*;

    #[test]
    fn local_inventory_is_conservative_and_never_invents_remote_peers() {
        let local_id = NodeId::from_bytes([7; 16]);
        let daemon =
            DaemonService::new(local_id, HealthLevel::Healthy, NodeCapabilities::default());
        let summary = daemon.local_node_summary().unwrap().unwrap();
        assert_eq!(summary.id, local_id);
        assert_eq!(summary.reachability, Reachability::Local);
        assert_eq!(summary.name.as_str(), "Local node");
        let remote = NodeId::from_bytes([8; 16]);
        assert_ne!(summary.id, remote);
    }

    #[test]
    fn daemon_without_local_identity_provides_no_synthetic_nodes() {
        let service = DaemonService::from_status(DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: None,
            current_network: None,
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        });
        assert_eq!(service.local_node_summary().unwrap(), None);
        assert_eq!(service.diagnostics_snapshot().unwrap().visible_nodes, 0);
    }

    #[test]
    fn diagnostics_only_counts_local_observation_until_p7_peer_authentication() {
        let daemon = DaemonService::new(
            NodeId::from_bytes([7; 16]),
            HealthLevel::Degraded,
            NodeCapabilities::default(),
        );
        let diagnostics = daemon.diagnostics_snapshot().unwrap();
        assert_eq!(diagnostics.visible_nodes, 1);
        assert_eq!(diagnostics.api_version, ApiVersion::CURRENT);
        assert_eq!(diagnostics.health, HealthLevel::Degraded);
        assert_eq!(diagnostics.current_network, None);
    }
}

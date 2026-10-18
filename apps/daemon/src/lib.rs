#![doc = "BlueRoute daemon D-Bus service implementation."]

// Prepared for the future authenticated transport runtime. Keep this
// fail-closed socket owner compiled and tested without treating it as wired.
mod approved_transport;
#[allow(dead_code)]
mod attempt_socket;
mod authorization;
mod control_listener;
mod control_tls;
mod join_network;
mod peer_pin_store;
mod peer_trust;
mod pinned_ingress;
mod registered_transport;
mod single_star;

use std::fmt;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use approved_transport::{
    ApprovedControlSetup, RegisteredControlSetupError, accept_approved_control_session,
    accept_registered_control_session, accept_registered_from_durable_approval,
    connect_approved_control_session, connect_registered_control_session,
};
pub use authorization::{
    CommandAuthorization, INTERNET_SHARING_ACTION_ID, MODIFY_ACTION_ID, command_authorization,
    command_operation,
};
use blueroute_core::{
    CoreError, DisplayName, ErrorKind, HealthLevel, NetworkId, NodeCapabilities, NodeId,
    Reachability,
};
use blueroute_protocol::peer_pin::SpkiSha256;
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DaemonStatus, DiagnosticFailure,
    DiagnosticSnapshot, Event, NodeSummary, ProtocolCodecError, Response, SystemDiagnosticMetadata,
    decode_command, encode_event, encode_response,
};
pub use control_listener::{
    CONTROL_TCP_PORT, bind_owned_nap_control_listener, owned_nap_control_address,
};
pub use control_tls::{
    ActiveControlSession, AuthenticatedControlPeer, ControlSessionError,
    ControlTlsCertificateCandidate, ControlTlsError, ControlTlsIdentityStore, ControlTlsStream,
    LocalControlTlsIdentity, PinnedControlTlsCertificate, PinnedControlTlsPeer,
    VerifiedControlTlsPeer, accept_pinned_control_tls, connect_pinned_control_tls,
    exchange_control_tls_candidate, exchange_control_tls_certificate, export_control_session_id,
    verify_control_tls_peer,
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
pub use registered_transport::{
    AuthenticatedNapTicket, RegisteredControlSession, RegisteredNapControlOwner,
    SessionAdmissionError,
};
pub use single_star::{
    LinuxStarHostRuntime, NetworkIdGenerator, NetworkOperationFuture, NetworkOperations,
    SingleStarNetworkOperations, StarHostRuntime, SystemNetworkIdGenerator, current_network,
};
use zbus::fdo;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, interface};

static NEXT_API_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Versioned local D-Bus interface implementation.
#[derive(Clone)]
pub struct DaemonService {
    status: Arc<Mutex<DaemonStatus>>,
    network_operations: Option<Arc<dyn NetworkOperations>>,
    peer_trust_operations: Option<Arc<dyn PeerTrustOperations>>,
    join_network_operations: Option<Arc<dyn JoinNetworkOperations>>,
    mutation_in_progress: Arc<AtomicBool>,
    system_diagnostics: Arc<Mutex<Option<SystemDiagnosticMetadata>>>,
    local_control_tls_fingerprint: Option<SpkiSha256>,
    recent_errors: Arc<Mutex<Vec<DiagnosticFailure>>>,
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
            system_diagnostics: Arc::new(Mutex::new(None)),
            local_control_tls_fingerprint: None,
            recent_errors: Arc::new(Mutex::new(Vec::new())),
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
            system_diagnostics: Arc::new(Mutex::new(None)),
            local_control_tls_fingerprint: None,
            recent_errors: Arc::new(Mutex::new(Vec::new())),
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
            system_diagnostics: Arc::new(Mutex::new(None)),
            local_control_tls_fingerprint: None,
            recent_errors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_join_network_operations(
        mut self,
        join_network_operations: Arc<dyn JoinNetworkOperations>,
    ) -> Self {
        self.join_network_operations = Some(join_network_operations);
        self
    }

    pub fn with_system_diagnostics(mut self, diagnostics: SystemDiagnosticMetadata) -> Self {
        self.system_diagnostics = Arc::new(Mutex::new(Some(diagnostics)));
        self
    }

    /// Expose only the daemon's persisted public-key fingerprint for
    /// independent comparison; this does not grant peer authorization.
    pub fn with_local_control_tls_fingerprint(mut self, fingerprint: SpkiSha256) -> Self {
        self.local_control_tls_fingerprint = Some(fingerprint);
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
        let system = self
            .system_diagnostics
            .lock()
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "BlueRoute system diagnostics are unavailable",
                    error.to_string(),
                )
            })?
            .clone();
        let recent_errors = self
            .recent_errors
            .lock()
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "BlueRoute recent diagnostics are unavailable",
                    error.to_string(),
                )
            })?
            .clone();
        Ok(DiagnosticSnapshot {
            api_version: status.api_version,
            health: status.health,
            current_network: status.current_network,
            visible_nodes: u32::from(status.local_node.is_some()),
            capabilities: status.capabilities,
            local_control_tls_spki_sha256: self
                .local_control_tls_fingerprint
                .map(SpkiSha256::to_canonical_hex),
            system,
            observed_peers: None,
            interfaces: None,
            routes: None,
            topology: None,
            recent_errors,
        })
    }

    fn record_recent_failure(&self, operation: &str, error: &fdo::Error) {
        let Ok(mut errors) = self.recent_errors.lock() else {
            return;
        };
        errors.push(DiagnosticFailure {
            timestamp_unix_ms: unix_time_ms(),
            operation: operation.to_owned(),
            failure_class: fdo_error_class(error).to_owned(),
        });
        if errors.len() > 16 {
            errors.remove(0);
        }
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
        let request_id = NEXT_API_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let command = match decode_command(payload) {
            Ok(command) => command,
            Err(_) => {
                eprintln!(
                    "{}",
                    format_api_log(
                        request_id,
                        "invalid_request",
                        "rejected",
                        Some("invalid_argument")
                    )
                );
                return Err(fdo::Error::InvalidArgs(
                    "malformed BlueRoute command payload".into(),
                ));
            }
        };
        let operation = command_operation(&command);
        let result = async {
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
                    let mut snapshot = self.diagnostics_snapshot().map_err(core_error_to_dbus)?;
                    if let Some(network) = &self.network_operations {
                        snapshot.interfaces = network
                            .diagnostic_interfaces()
                            .await
                            .map_err(core_error_to_dbus)?;
                    }
                    Self::encode_response(&Response::Diagnostics(Box::new(snapshot)))
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
                Command::LeaveNetwork => {
                    self.network_operations()?
                        .leave_network()
                        .await
                        .map_err(core_error_to_dbus)?;
                    self.update_current_network(None)?;
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
        .await;
        eprintln!(
            "{}",
            format_api_log(
                request_id,
                operation,
                if result.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                },
                result.as_ref().err().map(fdo_error_class)
            )
        );
        if let Err(error) = &result {
            self.record_recent_failure(operation, error);
        }
        result
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;
}

fn fdo_error_class(error: &fdo::Error) -> &'static str {
    match error {
        fdo::Error::AccessDenied(_) => "authorization_denied",
        fdo::Error::InvalidArgs(_) => "invalid_argument",
        fdo::Error::NotSupported(_) => "unsupported",
        fdo::Error::Failed(_) => "operation_failed",
        _ => "dbus_error",
    }
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn format_api_log(
    request_id: u64,
    operation: &str,
    outcome: &str,
    failure_class: Option<&str>,
) -> String {
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut record = serde_json::json!({
        "timestamp_unix_ms": timestamp_ms,
        "component": "daemon_api",
        "event": "request",
        "request_id": format!("api-{request_id:016x}"),
        "operation": operation,
        "outcome": outcome,
    });
    if let Some(failure_class) = failure_class {
        record["failure_class"] = serde_json::Value::String(failure_class.to_owned());
    }
    record.to_string()
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

    #[test]
    fn diagnostics_can_display_local_tls_public_key_without_granting_remote_trust() {
        let fingerprint = SpkiSha256::from_verified_certificate_digest([0xab; 32]).unwrap();
        let daemon = DaemonService::new(
            NodeId::from_bytes([7; 16]),
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        )
        .with_local_control_tls_fingerprint(fingerprint);
        let snapshot = daemon.diagnostics_snapshot().unwrap();
        assert_eq!(
            snapshot.local_control_tls_spki_sha256.as_deref(),
            Some("abababababababababababababababababababababababababababababababab")
        );
        assert!(snapshot.observed_peers.is_none());
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("PRIVATE KEY"));
        assert!(!encoded.contains("private_key"));
    }

    #[test]
    fn api_log_is_structured_correlatable_and_excludes_request_payloads() {
        let record: serde_json::Value = serde_json::from_str(&format_api_log(
            42,
            "create_network",
            "failed",
            Some("operation_failed"),
        ))
        .unwrap();
        assert_eq!(record["component"], "daemon_api");
        assert_eq!(record["event"], "request");
        assert_eq!(record["request_id"], "api-000000000000002a");
        assert_eq!(record["operation"], "create_network");
        assert_eq!(record["outcome"], "failed");
        assert_eq!(record["failure_class"], "operation_failed");
        assert!(record["timestamp_unix_ms"].as_u64().is_some());
        let serialized = record.to_string();
        assert!(!serialized.contains("Workshop"));
        assert!(!serialized.contains("private-key"));
    }

    #[test]
    fn recent_diagnostics_are_bounded_and_do_not_store_error_messages() {
        let daemon = DaemonService::new(
            NodeId::from_bytes([7; 16]),
            HealthLevel::Degraded,
            NodeCapabilities::default(),
        );
        for _ in 0..20 {
            daemon.record_recent_failure(
                "join_network",
                &fdo::Error::Failed("credential-or-user-data sentinel".into()),
            );
        }
        let snapshot = daemon.diagnostics_snapshot().unwrap();
        assert_eq!(snapshot.recent_errors.len(), 16);
        assert!(snapshot.recent_errors.iter().all(|error| {
            error.operation == "join_network" && error.failure_class == "operation_failed"
        }));
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("credential-or-user-data sentinel")
        );
    }
}

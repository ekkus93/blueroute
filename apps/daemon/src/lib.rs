#![doc = "BlueRoute daemon D-Bus service implementation."]

// Prepared for the future authenticated transport runtime. Keep this
// fail-closed socket owner compiled and tested without treating it as wired.
mod approved_transport;
#[allow(dead_code)]
mod attempt_socket;
mod authorization;
mod client_leave;
mod client_link;
mod client_reconcile;
mod client_reconnect;
mod client_record;
mod client_runtime;
mod control_listener;
mod control_supervisor;
mod control_tls;
mod control_worker;
mod device_name;
mod enrollment_transport;
mod join_authorization;
mod join_control;
mod join_network;
mod outbound_control;
mod peer_locator;
mod peer_pin_store;
mod peer_trust;
mod pending_join;
mod pinned_ingress;
mod registered_transport;
mod runtime_health;
mod single_star;
mod state_publisher;

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
    ADDRESS_CONFLICT_ERROR, ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH,
    DaemonStatus, DiagnosticFailure, DiagnosticSnapshot, Event, NodeSummary, ProtocolCodecError,
    Response, SystemDiagnosticMetadata, decode_command, encode_event, encode_response,
};
pub use client_leave::{
    ClientLeaveBackend, SystemClientLeave, leave_observed, remove_owned_network_state,
};
pub use client_link::{ClientReconnector, health_for_client_link};
pub use client_reconcile::{
    ClientHost, ClientReconcileOutcome, ClientRuntime, ObservedClient, reconcile_client_once,
    single_approved_host,
};
pub use client_reconnect::{ReconnectEnd, ReconnectPolicy, ReconnectProgress, run_reconnect};
pub use client_record::{ClientRecord, ClientRecordStore};
pub use client_runtime::SystemClientRuntime;
pub use control_listener::{
    CONTROL_TCP_PORT, ObservedControlListener, bind_owned_nap_control_listener,
    owned_nap_control_address,
};
pub use control_supervisor::{ControlSupervisors, Established, SupervisedState};
pub use control_tls::{
    ActiveControlSession, AuthenticatedControlPeer, ControlSessionError,
    ControlTlsCertificateCandidate, ControlTlsError, ControlTlsIdentityStore, ControlTlsStream,
    LocalControlTlsIdentity, PinnedControlTlsCertificate, PinnedControlTlsPeer,
    VerifiedControlTlsPeer, accept_pinned_control_tls, connect_pinned_control_tls,
    exchange_control_tls_candidate, exchange_control_tls_certificate, export_control_session_id,
    verify_control_tls_peer,
};
pub use control_worker::{
    AuthenticatedControlPeers, DaemonControlWorker, spawn_daemon_control_worker,
    spawn_daemon_control_worker_with_peers,
};
pub use device_name::DeviceNameStore;
pub use enrollment_transport::enroll_confirmed_control_candidate;
pub use join_authorization::{
    JoinCommitAuthorization, JoinCommitReceipt, PendingHostCommitAuthorization,
};
pub use join_control::{JoinControlConfig, run_join_control};
pub use join_network::{
    EXPERIMENTAL_JOIN, JoinControlSession, JoinIpLease, JoinNetworkFuture, JoinNetworkOperations,
    JoinPanLink, JoinRuntime, LinuxJoinRuntime, TransactionalJoinNetworkOperations,
};
pub use outbound_control::{
    PanuControlRoute, connect_registered_panu_control_session, planned_panu_control_endpoint,
    run_registered_panu_supervision_until_stopped, supervise_registered_panu_control_session,
};
pub use peer_locator::{BluezPeerLocator, PeerLocator, select_single_peer};
pub use peer_pin_store::PeerPinStore;
pub use peer_trust::{
    DurablePeerTrustOperations, PeerTrustFuture, PeerTrustOperations, pair_and_trust_bluetooth_peer,
};
pub use pending_join::{PendingJoinHost, PendingJoinOperations, PendingJoinStore};
pub use pinned_ingress::{PinnedIngressError, PinnedSessionIngress, VerifiedTlsIdentity};
pub use registered_transport::{
    AuthenticatedNapTicket, RegisteredControlSession, RegisteredNapControlOwner,
    SessionAdmissionError,
};
pub use runtime_health::{
    DaemonHealthWorker, spawn_daemon_capability_health_worker, spawn_daemon_runtime_worker,
    system_diagnostics_from_report,
};
pub use single_star::{
    LinuxStarHostRuntime, NetworkIdGenerator, NetworkOperationFuture, NetworkOperations,
    SingleStarNetworkOperations, StarHostRuntime, SystemNetworkIdGenerator, current_network,
};
pub use state_publisher::{
    NETWORK_RECHECK, STATE_RECHECK, StatePublisher, StatePublisherWorker, spawn_state_publisher,
};
use zbus::fdo;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, interface};

/// The two independent inputs to published health: what the capability monitor observes and
/// what reconnecting a joined client implies. Published health is the worse of the two.
struct HealthInputs {
    observed: HealthLevel,
    client: HealthLevel,
}

impl HealthInputs {
    const fn new(observed: HealthLevel) -> Self {
        Self {
            observed,
            client: HealthLevel::Healthy,
        }
    }

    fn merged(&self) -> HealthLevel {
        self.observed.max(self.client)
    }
}

static NEXT_API_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Versioned local D-Bus interface implementation.
#[derive(Clone)]
pub struct DaemonService {
    status: Arc<Mutex<DaemonStatus>>,
    network_operations: Option<Arc<dyn NetworkOperations>>,
    peer_trust_operations: Option<Arc<dyn PeerTrustOperations>>,
    join_network_operations: Option<Arc<dyn JoinNetworkOperations>>,
    pending_join_operations: Option<Arc<PendingJoinOperations>>,
    client_reconnector: Option<Arc<ClientReconnector>>,
    control_peers: Option<AuthenticatedControlPeers>,
    /// The current display name and where it is kept (P12-009). A label only, never identity.
    device_name: Arc<Mutex<Option<DisplayName>>>,
    device_name_store: Option<Arc<DeviceNameStore>>,
    health_inputs: Arc<Mutex<HealthInputs>>,
    mutation_in_progress: Arc<AtomicBool>,
    system_diagnostics: Arc<Mutex<Option<SystemDiagnosticMetadata>>>,
    local_control_tls_fingerprint: Option<SpkiSha256>,
    recent_errors: Arc<Mutex<Vec<DiagnosticFailure>>>,
    /// Where every observable change is announced (CR-F1).
    state_publisher: Arc<StatePublisher>,
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
        let health_now = status.health;
        Self {
            status: Arc::new(Mutex::new(status)),
            network_operations: None,
            peer_trust_operations: None,
            join_network_operations: None,
            pending_join_operations: None,
            client_reconnector: None,
            control_peers: None,
            device_name: Arc::new(Mutex::new(None)),
            device_name_store: None,
            health_inputs: Arc::new(Mutex::new(HealthInputs::new(health_now))),
            mutation_in_progress: Arc::new(AtomicBool::new(false)),
            system_diagnostics: Arc::new(Mutex::new(None)),
            local_control_tls_fingerprint: None,
            recent_errors: Arc::new(Mutex::new(Vec::new())),
            state_publisher: Arc::default(),
        }
    }

    pub fn with_network_operations(
        status: DaemonStatus,
        network_operations: Arc<dyn NetworkOperations>,
    ) -> Self {
        let health_now = status.health;
        Self {
            status: Arc::new(Mutex::new(status)),
            network_operations: Some(network_operations),
            peer_trust_operations: None,
            join_network_operations: None,
            pending_join_operations: None,
            client_reconnector: None,
            control_peers: None,
            device_name: Arc::new(Mutex::new(None)),
            device_name_store: None,
            health_inputs: Arc::new(Mutex::new(HealthInputs::new(health_now))),
            mutation_in_progress: Arc::new(AtomicBool::new(false)),
            system_diagnostics: Arc::new(Mutex::new(None)),
            local_control_tls_fingerprint: None,
            recent_errors: Arc::new(Mutex::new(Vec::new())),
            state_publisher: Arc::default(),
        }
    }

    pub fn with_operations(
        status: DaemonStatus,
        network_operations: Arc<dyn NetworkOperations>,
        peer_trust_operations: Arc<dyn PeerTrustOperations>,
    ) -> Self {
        let health_now = status.health;
        Self {
            status: Arc::new(Mutex::new(status)),
            network_operations: Some(network_operations),
            peer_trust_operations: Some(peer_trust_operations),
            join_network_operations: None,
            pending_join_operations: None,
            client_reconnector: None,
            control_peers: None,
            device_name: Arc::new(Mutex::new(None)),
            device_name_store: None,
            health_inputs: Arc::new(Mutex::new(HealthInputs::new(health_now))),
            mutation_in_progress: Arc::new(AtomicBool::new(false)),
            system_diagnostics: Arc::new(Mutex::new(None)),
            local_control_tls_fingerprint: None,
            recent_errors: Arc::new(Mutex::new(Vec::new())),
            state_publisher: Arc::default(),
        }
    }

    pub fn with_join_network_operations(
        mut self,
        join_network_operations: Arc<dyn JoinNetworkOperations>,
    ) -> Self {
        self.join_network_operations = Some(join_network_operations);
        self
    }

    pub fn with_pending_join_operations(
        mut self,
        pending_join_operations: Arc<PendingJoinOperations>,
    ) -> Self {
        self.pending_join_operations = Some(pending_join_operations);
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

    /// Refresh the public runtime capability and system observation consumed
    /// by status, capability, and diagnostic D-Bus queries. Do not mutate
    /// membership, enrolled keys, existing PAN ownership, or routing.
    ///
    /// Update both locks in the same order used by diagnostics_snapshot to
    /// keep each published snapshot coherent even across service restarts.
    /// If the probe fails entirely, None means diagnostics are unavailable,
    /// not that an empty adapter inventory was independently observed.
    pub fn update_runtime_observation(
        &self,
        observed_health: HealthLevel,
        observed_capabilities: NodeCapabilities,
        observed_system: Option<SystemDiagnosticMetadata>,
    ) -> Result<bool, CoreError> {
        let mut status = self.status.lock().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "BlueRoute daemon status lock was poisoned",
                error.to_string(),
            )
        })?;
        let mut system = self.system_diagnostics.lock().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "BlueRoute system diagnostics lock was poisoned",
                error.to_string(),
            )
        })?;
        let observed_health = self.merge_observed_health(observed_health)?;
        let changed = status.health != observed_health
            || status.capabilities != observed_capabilities
            || *system != observed_system;
        status.health = observed_health;
        status.capabilities = observed_capabilities;
        *system = observed_system;
        drop(system);
        drop(status);
        if changed {
            self.state_publisher.observed();
        }
        Ok(changed)
    }

    /// Publish a fresh capability-health observation to all cloned daemon D-Bus
    /// service handles. This does not change membership, authorize a peer, or
    /// claim that an existing PAN attachment has been restored.
    pub fn update_runtime_health(&self, observed: HealthLevel) -> Result<bool, CoreError> {
        let mut status = self.status.lock().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "BlueRoute daemon health status lock was poisoned",
                error.to_string(),
            )
        })?;
        let observed = self.merge_observed_health(observed)?;
        let changed = status.health != observed;
        status.health = observed;
        drop(status);
        if changed {
            self.state_publisher.observed();
        }
        Ok(changed)
    }

    /// Record the capability-observed health and return what to publish: the worse of it and
    /// what the joined-client reconnection implies. Always called with the status lock held,
    /// so the lock order is status then health inputs.
    fn merge_observed_health(&self, observed: HealthLevel) -> Result<HealthLevel, CoreError> {
        let mut inputs = self.health_inputs.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::Internal,
                "BlueRoute health inputs lock was poisoned",
            )
        })?;
        inputs.observed = observed;
        Ok(inputs.merged())
    }

    /// Let a joined client's reconnection state feed daemon health. The observer touches only
    /// the shared status and health inputs, so it holds no reference back to the service.
    /// Run one already-authorized command. The D-Bus `request` method decodes, authorizes and
    /// logs; this is the dispatch, separate so it can be tested without a bus.
    async fn execute(&self, command: Command) -> Result<String, RequestError> {
        match command {
            Command::CreateNetwork { name } => {
                let network = self
                    .network_operations()?
                    .create_network(name)
                    .await
                    .map_err(core_error_to_request)?;
                self.update_current_network(Some(network))?;
                Ok(Self::encode_response(&Response::Ack)?)
            }
            Command::JoinNetwork { network } => {
                self.join_network_operations()?
                    .join_network(network)
                    .await
                    .map_err(core_error_to_request)?;
                self.update_current_network(Some(network))?;
                // Watch the new session so a later link loss is reconnected (P6-010). The
                // join already succeeded, so a failure here only loses that watch.
                if let Some(reconnector) = &self.client_reconnector
                    && let Err(error) = reconnector.joined(network)
                {
                    eprintln!("BlueRoute could not watch the joined network's link: {error}");
                }
                Ok(Self::encode_response(&Response::Ack)?)
            }
            other => self
                .execute_standard(other)
                .await
                .map_err(RequestError::from),
        }
    }

    /// Every command whose failures map to standard D-Bus errors.
    async fn execute_standard(&self, command: Command) -> fdo::Result<String> {
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
            Command::ListNodes => Self::encode_response(&Response::Nodes(self.node_summaries()?)),
            Command::GetNode { node } => {
                let matching = self
                    .node_summaries()?
                    .into_iter()
                    .find(|entry| entry.id == node);
                Self::encode_response(&Response::Node(matching))
            }
            Command::GetClientLink => Self::encode_response(&Response::ClientLink(
                self.client_reconnector
                    .as_ref()
                    .and_then(|reconnector| reconnector.state()),
            )),
            Command::RetryClientReconnect => {
                let reconnector = self.client_reconnector.as_ref().ok_or_else(|| {
                    fdo::Error::NotSupported(
                        "client reconnection is unavailable in this daemon instance".into(),
                    )
                })?;
                reconnector.retry().map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Ack)
            }
            Command::LeaveNetwork => {
                // Stop reconnecting first so a run cannot bring a session back while the
                // leave tears it down.
                if let Some(reconnector) = &self.client_reconnector {
                    reconnector.cancel();
                }
                // A node that joined someone else's network must reverse its join; only
                // otherwise is this a host leave.
                let left_as_client = match &self.join_network_operations {
                    Some(join) => join.leave_joined().await.map_err(core_error_to_dbus)?,
                    None => false,
                };
                if !left_as_client {
                    self.network_operations()?
                        .leave_network()
                        .await
                        .map_err(core_error_to_dbus)?;
                }
                self.update_current_network(None)?;
                if let Some(reconnector) = &self.client_reconnector {
                    reconnector.cleared();
                }
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
            Command::EnrollPeerKey { node, fingerprint } => {
                self.ensure_not_local_node(node)?;
                let fingerprint = SpkiSha256::parse_canonical_hex(&fingerprint).map_err(|_| {
                    fdo::Error::InvalidArgs(
                        "peer key fingerprint must be canonical lowercase SHA-256 hex".into(),
                    )
                })?;
                self.peer_trust_operations()?
                    .enroll_peer_key(node, fingerprint)
                    .await
                    .map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Ack)
            }
            Command::RecordJoinHost {
                network,
                node,
                fingerprint,
            } => {
                self.ensure_not_local_node(node)?;
                let fingerprint = SpkiSha256::parse_canonical_hex(&fingerprint).map_err(|_| {
                    fdo::Error::InvalidArgs(
                        "join host fingerprint must be canonical lowercase SHA-256 hex".into(),
                    )
                })?;
                let host =
                    PendingJoinHost::new(network, node, fingerprint).map_err(core_error_to_dbus)?;
                self.pending_join_operations()?
                    .record_host(host)
                    .map_err(core_error_to_dbus)?;
                Self::encode_response(&Response::Ack)
            }
            Command::ClearJoinHost => {
                self.pending_join_operations()?
                    .clear()
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
            Command::SetDeviceName { name } => {
                let store = self.device_name_store.as_ref().ok_or_else(|| {
                    fdo::Error::NotSupported(
                        "renaming is unavailable in this daemon instance".into(),
                    )
                })?;
                // Durable first, so a failed write never shows a name that a restart loses.
                store.save(&name).map_err(core_error_to_dbus)?;
                if let Ok(mut current) = self.device_name.lock() {
                    *current = Some(name);
                }
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

    /// Keep this computer's display name in `store`, starting from `current` (P12-009).
    pub fn with_device_name_store(
        mut self,
        store: Arc<DeviceNameStore>,
        current: DisplayName,
    ) -> Self {
        self.device_name = Arc::new(Mutex::new(Some(current)));
        self.device_name_store = Some(store);
        self
    }

    /// Report the host control worker's authenticated peers in diagnostics (P6-007).
    pub fn with_control_peers(mut self, peers: AuthenticatedControlPeers) -> Self {
        self.control_peers = Some(peers);
        self
    }

    pub fn with_client_reconnector(mut self, reconnector: Arc<ClientReconnector>) -> Self {
        let status = self.status.clone();
        let inputs = self.health_inputs.clone();
        let publisher = self.state_publisher.clone();
        reconnector.set_observer(Arc::new(move |state| {
            {
                let Ok(mut status) = status.lock() else {
                    return;
                };
                let Ok(mut inputs) = inputs.lock() else {
                    return;
                };
                inputs.client = health_for_client_link(state);
                status.health = inputs.merged();
            }
            // Every link phase is announced, even when the merged health stays the same.
            publisher.observed();
        }));
        self.client_reconnector = Some(reconnector);
        self
    }

    /// Where this service announces observable changes; shared by every clone.
    pub fn state_publisher(&self) -> Arc<StatePublisher> {
        self.state_publisher.clone()
    }

    /// Everything a client can read from memory, as one comparable value: status, diagnostics
    /// (health, capabilities, system, authenticated peers, recent failures), nodes and names,
    /// and the joined client's link. `None` only if a lock is poisoned.
    pub(crate) fn observable_state(&self) -> Option<String> {
        let status = encode_response(&Response::Status(self.status_snapshot().ok()?)).ok()?;
        let diagnostics = encode_response(&Response::Diagnostics(Box::new(
            self.diagnostics_snapshot().ok()?,
        )))
        .ok()?;
        let nodes = encode_response(&Response::Nodes(self.node_summaries().ok()?)).ok()?;
        let link = encode_response(&Response::ClientLink(
            self.client_reconnector
                .as_ref()
                .and_then(|reconnector| reconnector.state()),
        ))
        .ok()?;
        Some([status, diagnostics, nodes, link].join("\n"))
    }

    /// The network list as `ListNetworks` reports it. `None` when it cannot be read; a read that
    /// starts failing or recovers is itself a change worth announcing.
    pub(crate) async fn observable_networks(&self) -> Option<String> {
        let networks = self
            .network_operations
            .as_ref()?
            .list_networks()
            .await
            .ok()?;
        encode_response(&Response::Networks(networks)).ok()
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

    fn pending_join_operations(&self) -> fdo::Result<&Arc<PendingJoinOperations>> {
        self.pending_join_operations.as_ref().ok_or_else(|| {
            fdo::Error::NotSupported(
                "pending-join operations are unavailable in this daemon instance".into(),
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
    /// The local node, then (on a host) every peer holding an authenticated control session.
    /// Those peers proved their pinned key over mutual TLS with current approval, and on a
    /// single star their session runs over the PANU link to this host, so they are direct links.
    /// Nothing else (discovery, trust alone, advertisements) ever becomes a node here.
    fn node_summaries(&self) -> fdo::Result<Vec<NodeSummary>> {
        let local = self.local_node_summary()?;
        let local_id = local.as_ref().map(|node| node.id);
        let mut nodes: Vec<NodeSummary> = local.into_iter().collect();
        if let Some(peers) = &self.control_peers {
            for peer in peers.snapshot() {
                if Some(peer) == local_id {
                    continue;
                }
                nodes.push(NodeSummary {
                    id: peer,
                    name: authenticated_peer_name(peer).map_err(core_error_to_dbus)?,
                    reachability: Reachability::DirectLink,
                });
            }
        }
        Ok(nodes)
    }

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
            name: match self.device_name.lock().ok().and_then(|name| name.clone()) {
                Some(name) => name,
                None => DisplayName::new("Local node").map_err(core_error_to_dbus)?,
            },
            reachability: Reachability::Local,
        }))
    }

    /// Read-only diagnostics never invent members from approvals, Bluetooth
    /// discovery or unverified peer-controlled Hello fields.
    pub fn diagnostics_snapshot(&self) -> Result<DiagnosticSnapshot, CoreError> {
        // Hold both observation locks while cloning so a concurrent 15-second
        // capability refresh cannot mix the new health with an old adapter
        // inventory (or vice versa). The writer takes the same lock order.
        let (status, system) = {
            let status = self.status.lock().map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "BlueRoute daemon status lock was poisoned",
                    error.to_string(),
                )
            })?;
            let system = self.system_diagnostics.lock().map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::Internal,
                    "BlueRoute system diagnostics are unavailable",
                    error.to_string(),
                )
            })?;
            (status.clone(), system.clone())
        };
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
            // The same nodes `ListNodes` reports: this computer plus authenticated peers.
            visible_nodes: u32::from(status.local_node.is_some())
                + self.control_peers.as_ref().map_or(0, |peers| {
                    let local = status.local_node;
                    u32::try_from(
                        peers
                            .snapshot()
                            .into_iter()
                            .filter(|peer| Some(*peer) != local)
                            .count(),
                    )
                    .unwrap_or(u32::MAX)
                }),
            capabilities: status.capabilities,
            local_control_tls_spki_sha256: self
                .local_control_tls_fingerprint
                .map(SpkiSha256::to_canonical_hex),
            system,
            observed_peers: None,
            interfaces: None,
            routes: None,
            topology: None,
            authenticated_control_peers: self
                .control_peers
                .as_ref()
                .map(AuthenticatedControlPeers::snapshot),
            recent_errors,
        })
    }

    fn record_recent_failure(&self, operation: &str, error: &RequestError) {
        let Ok(mut errors) = self.recent_errors.lock() else {
            return;
        };
        errors.push(DiagnosticFailure {
            timestamp_unix_ms: unix_time_ms(),
            operation: operation.to_owned(),
            failure_class: error.class().to_owned(),
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
    ) -> Result<String, RequestError> {
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
                return Err(
                    fdo::Error::InvalidArgs("malformed BlueRoute command payload".into()).into(),
                );
            }
        };
        let operation = command_operation(&command);
        let mutating = matches!(
            authorization::command_authorization(&command),
            CommandAuthorization::PolicyKit(_)
        );
        let result = async {
            authorization::authorize_command(connection, &header, &command).await?;
            let _mutation = mutating
                .then(|| MutationGuard::claim(&self.mutation_in_progress))
                .transpose()?;

            self.execute(command).await
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
                result.as_ref().err().map(RequestError::class)
            )
        );
        if let Err(error) = &result {
            self.record_recent_failure(operation, error);
        }
        // After every state update the request made, and outside every lock. A mutation is
        // announced whether or not it succeeded; a failed read changes only recent failures,
        // which the publisher notices by itself.
        if mutating {
            self.state_publisher.committed();
        } else if result.is_err() {
            self.state_publisher.observed();
        }
        result
    }

    #[zbus(signal)]
    async fn event(emitter: &SignalEmitter<'_>, payload: &str) -> zbus::Result<()>;

    /// API 1.7: something observable changed; reread. See `STATE_CHANGED_SIGNAL`.
    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, revision: u64) -> zbus::Result<()>;
}

/// What a request can fail with on the bus. Standard errors keep their standard names; failures a
/// client should handle specifically get a BlueRoute name. Implemented by hand rather than with
/// zbus's derive: the derive sends a wrapped standard error as the generic
/// `org.freedesktop.zbus.Error`, which would hide `AccessDenied` and `NotSupported` from clients.
#[derive(Debug)]
pub enum RequestError {
    Standard(fdo::Error),
    /// The network's private IPv4 range overlaps one already in use on this computer.
    AddressConflict(String),
}

impl From<fdo::Error> for RequestError {
    fn from(error: fdo::Error) -> Self {
        Self::Standard(error)
    }
}

impl From<zbus::Error> for RequestError {
    fn from(error: zbus::Error) -> Self {
        Self::Standard(error.into())
    }
}

impl zbus::DBusError for RequestError {
    fn name(&self) -> zbus::names::ErrorName<'_> {
        match self {
            Self::Standard(error) => error.name(),
            Self::AddressConflict(_) => {
                zbus::names::ErrorName::from_static_str_unchecked(ADDRESS_CONFLICT_ERROR)
            }
        }
    }

    fn description(&self) -> Option<&str> {
        match self {
            Self::Standard(error) => error.description(),
            Self::AddressConflict(message) => Some(message),
        }
    }

    fn create_reply(&self, call: &zbus::message::Header<'_>) -> zbus::Result<zbus::Message> {
        match self {
            Self::Standard(error) => error.create_reply(call),
            Self::AddressConflict(message) => {
                zbus::Message::error(call, self.name())?.build(&(message.as_str(),))
            }
        }
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        use zbus::DBusError;
        write!(
            formatter,
            "{}: {}",
            self.name(),
            self.description().unwrap_or("no description")
        )
    }
}

impl std::error::Error for RequestError {}

impl RequestError {
    /// The coarse class for logs and recent-failure diagnostics; never the message.
    fn class(&self) -> &'static str {
        match self {
            Self::AddressConflict(_) => "address_conflict",
            Self::Standard(error) => fdo_error_class(error),
        }
    }

    #[cfg(test)]
    fn fdo(&self) -> Option<&fdo::Error> {
        match self {
            Self::Standard(error) => Some(error),
            Self::AddressConflict(_) => None,
        }
    }
}

/// As `core_error_to_dbus`, but address conflicts get their own error name.
fn core_error_to_request(error: CoreError) -> RequestError {
    if error.kind() == ErrorKind::AddressConflict {
        RequestError::AddressConflict(error.message().to_owned())
    } else {
        core_error_to_dbus(error).into()
    }
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

/// The control protocol carries no display name, so an authenticated peer is labelled by the
/// start of its NodeId. The label is never used to identify or authorize anything.
fn authenticated_peer_name(peer: NodeId) -> Result<DisplayName, CoreError> {
    let id = peer.to_string();
    DisplayName::new(format!("Device {}", &id[..8]))
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

    fn decode(json: &str) -> Response {
        blueroute_protocol::decode_response(json).unwrap()
    }

    #[test]
    fn renaming_persists_changes_only_the_label_and_survives_a_new_service() {
        futures_lite::future::block_on(async {
            let dir = std::env::temp_dir().join(format!(
                "blueroute-rename-{}-{}",
                std::process::id(),
                line!()
            ));
            let local = NodeId::from_bytes([0x71; 16]);
            let service =
                || DaemonService::new(local, HealthLevel::Healthy, NodeCapabilities::default());
            let store = Arc::new(DeviceNameStore::new(&dir));
            let named = service().with_device_name_store(
                store.clone(),
                DisplayName::new("BlueRoute Device").unwrap(),
            );
            let name = DisplayName::new("Kitchen laptop").unwrap();
            let ack = named
                .execute(Command::SetDeviceName { name: name.clone() })
                .await
                .unwrap();
            assert_eq!(decode(&ack), Response::Ack);
            let Response::Nodes(nodes) = decode(&named.execute(Command::ListNodes).await.unwrap())
            else {
                panic!("expected nodes");
            };
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].name, name);
            // Identity is untouched: same NodeId as before the rename.
            assert_eq!(nodes[0].id, local);
            assert_eq!(named.status_snapshot().unwrap().local_node, Some(local));
            // Durable: what a restarted daemon would load.
            assert_eq!(store.load().unwrap(), Some(name));

            // A daemon without a name store refuses rather than pretending.
            let refused = service()
                .execute(Command::SetDeviceName {
                    name: DisplayName::new("x").unwrap(),
                })
                .await
                .unwrap_err();
            assert!(matches!(refused.fdo(), Some(fdo::Error::NotSupported(_))));
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    #[test]
    fn authenticated_control_peers_are_listed_as_direct_nodes_and_nothing_else_is() {
        futures_lite::future::block_on(async {
            let local = NodeId::from_bytes([0x73; 16]);
            let peer = NodeId::from_bytes([0xab; 16]);
            let peers = AuthenticatedControlPeers::default();
            let service =
                DaemonService::new(local, HealthLevel::Healthy, NodeCapabilities::default())
                    .with_control_peers(peers.clone());
            let nodes = || async {
                let Response::Nodes(nodes) =
                    decode(&service.execute(Command::ListNodes).await.unwrap())
                else {
                    panic!("expected nodes");
                };
                nodes
            };
            // No session: only this computer.
            assert_eq!(nodes().await.len(), 1);
            assert_eq!(service.diagnostics_snapshot().unwrap().visible_nodes, 1);

            // An authenticated peer appears as a direct link with a neutral, ID-derived label.
            peers.set_for_tests(vec![peer]);
            let listed = nodes().await;
            assert_eq!(listed.len(), 2);
            assert_eq!(listed[0].reachability, Reachability::Local);
            assert_eq!(listed[1].id, peer);
            assert_eq!(listed[1].reachability, Reachability::DirectLink);
            assert_eq!(listed[1].name.as_str(), "Device abababab");
            assert_eq!(service.diagnostics_snapshot().unwrap().visible_nodes, 2);
            let Response::Node(Some(found)) = decode(
                &service
                    .execute(Command::GetNode { node: peer })
                    .await
                    .unwrap(),
            ) else {
                panic!("an authenticated peer must be found");
            };
            assert_eq!(found.id, peer);

            // When the session ends, the peer is gone again; unknown IDs are never invented.
            peers.set_for_tests(Vec::new());
            assert_eq!(nodes().await.len(), 1);
            let Response::Node(None) = decode(
                &service
                    .execute(Command::GetNode { node: peer })
                    .await
                    .unwrap(),
            ) else {
                panic!("a peer without a session must not be listed");
            };
            // The local node is never duplicated even if it appeared in the peer list.
            peers.set_for_tests(vec![local]);
            assert_eq!(nodes().await.len(), 1);
        });
    }

    /// Creating always fails with an address conflict; nothing else is used.
    struct ConflictingNetworks;

    impl NetworkOperations for ConflictingNetworks {
        fn create_network(&self, _name: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
            Box::pin(async {
                Err(CoreError::new(
                    ErrorKind::AddressConflict,
                    "the network's address range is already in use on this computer",
                ))
            })
        }
        fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
            Box::pin(async {
                Err(CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "fake refusal for name checks",
                ))
            })
        }
        fn list_networks(
            &self,
        ) -> NetworkOperationFuture<'_, Vec<blueroute_protocol::NetworkSummary>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn an_address_conflict_has_its_own_error_name_and_others_keep_theirs() {
        use zbus::DBusError;
        futures_lite::future::block_on(async {
            let mut service = DaemonService::new(
                NodeId::from_bytes([0x74; 16]),
                HealthLevel::Healthy,
                NodeCapabilities::default(),
            );
            service.network_operations = Some(Arc::new(ConflictingNetworks));
            let conflict = service
                .execute(Command::CreateNetwork {
                    name: DisplayName::new("Lab").unwrap(),
                })
                .await
                .unwrap_err();
            assert_eq!(
                conflict.name().as_str(),
                blueroute_protocol::ADDRESS_CONFLICT_ERROR
            );
            assert_eq!(conflict.class(), "address_conflict");
            assert!(
                conflict
                    .description()
                    .is_some_and(|text| text.contains("already in use"))
            );
            // Standard failures keep their standard D-Bus names through the same type.
            let denied = service.execute(Command::LeaveNetwork).await.unwrap_err();
            assert_eq!(
                denied.name().as_str(),
                "org.freedesktop.DBus.Error.AccessDenied"
            );
            assert_eq!(denied.class(), "authorization_denied");
        });
    }

    #[test]
    fn authenticated_peer_arrival_and_departure_change_the_published_snapshot() {
        // The control worker does not mark the publisher; its 250 ms recheck compares this
        // snapshot, so peer arrival, departure and revocation must all show in it (CR-F1-04).
        let peers = AuthenticatedControlPeers::default();
        let service = DaemonService::new(
            NodeId::from_bytes([0x71; 16]),
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        )
        .with_control_peers(peers.clone());
        let alone = service.observable_state().unwrap();
        peers.set_for_tests(vec![NodeId::from_bytes([0x72; 16])]);
        let joined = service.observable_state().unwrap();
        assert_ne!(alone, joined);
        peers.set_for_tests(Vec::new());
        assert_eq!(service.observable_state().unwrap(), alone);
    }

    #[test]
    fn client_link_dispatch_without_a_reconnector_reads_none_and_refuses_retry() {
        futures_lite::future::block_on(async {
            let service = DaemonService::new(
                NodeId::from_bytes([0x72; 16]),
                HealthLevel::Healthy,
                NodeCapabilities::default(),
            );
            let link = service.execute(Command::GetClientLink).await.unwrap();
            assert_eq!(decode(&link), Response::ClientLink(None));
            let retry = service
                .execute(Command::RetryClientReconnect)
                .await
                .unwrap_err();
            assert!(matches!(retry.fdo(), Some(fdo::Error::NotSupported(_))));
        });
    }

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
    fn concurrent_runtime_refresh_cannot_mix_health_and_adapter_generations() {
        use blueroute_core::{CapabilitySource, Sourced};

        let node = NodeId::from_bytes([7; 16]);
        let service = DaemonService::new(node, HealthLevel::Healthy, NodeCapabilities::default());
        let observed = |powered: bool| {
            let health = if powered {
                HealthLevel::Healthy
            } else {
                HealthLevel::Degraded
            };
            let capabilities = NodeCapabilities {
                adapter_usable: Some(Sourced::new(powered, CapabilitySource::Discovered)),
                ..NodeCapabilities::default()
            };
            let system = SystemDiagnosticMetadata {
                daemon_version: "test".into(),
                platform: "Linux".into(),
                kernel_release: None,
                bluez_version: Some("test".into()),
                network_backend_version: None,
                adapters: vec![blueroute_protocol::DiagnosticAdapter {
                    id: "hci0".into(),
                    powered,
                    address: None,
                    driver: None,
                }],
            };
            (health, capabilities, system)
        };
        let (health, capabilities, system) = observed(true);
        service
            .update_runtime_observation(health, capabilities, Some(system))
            .unwrap();
        let writer = service.clone();
        let updater = std::thread::spawn(move || {
            for index in 0..1000 {
                let (health, capabilities, system) = observed(index % 2 == 0);
                writer
                    .update_runtime_observation(health, capabilities, Some(system))
                    .unwrap();
            }
        });
        for _ in 0..1000 {
            let snapshot = service.diagnostics_snapshot().unwrap();
            let system = snapshot.system.unwrap();
            let powered = system.adapters[0].powered;
            assert_eq!(snapshot.capabilities.adapter_usable.unwrap().value, powered);
            assert_eq!(
                snapshot.health,
                if powered {
                    HealthLevel::Healthy
                } else {
                    HealthLevel::Degraded
                }
            );
        }
        updater.join().unwrap();
        assert_eq!(service.status_snapshot().unwrap().local_node, Some(node));
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
                &fdo::Error::Failed("credential-or-user-data sentinel".into()).into(),
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

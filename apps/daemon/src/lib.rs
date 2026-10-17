#![doc = "BlueRoute daemon D-Bus service implementation."]

// Prepared for the future authenticated transport runtime. Keep this
// fail-closed socket owner compiled and tested without treating it as wired.
mod approved_transport;
mod nap_control_admission;
#[allow(dead_code)]
mod attempt_socket;
mod authorization;
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
pub use nap_control_admission::{
    AuthenticatedNapTicket, activate_authenticated_nap, reserve_authenticated_nap,
    retire_authenticated_nap,
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
pub use registered_transport::{RegisteredControlSession, SessionAdmissionError};
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

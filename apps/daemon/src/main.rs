use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blueroute_core::{DaemonConfig, HealthLevel, MembershipRole};
use blueroute_daemon::{
    ClientReconnector, ClientRecordStore, ControlTlsIdentityStore, DaemonService,
    DurablePeerTrustOperations, JoinControlConfig, NetworkOperations, PeerPinStore,
    PendingHostCommitAuthorization, PendingJoinOperations, PendingJoinStore, ReconnectPolicy,
    SingleStarNetworkOperations, TransactionalJoinNetworkOperations, current_network,
    spawn_daemon_capability_health_worker, spawn_daemon_control_worker,
    system_diagnostics_from_report,
};
use blueroute_linux::{
    NetworkMembershipStore, NodeIdentityStore, SystemCapabilityProbe, SystemSupportLevel,
};
use blueroute_protocol::session::ControlSessionRegistry;
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus};
use futures_lite::future;
use zbus::connection::Builder;

const DEFAULT_STATE_DIRECTORY: &str = "/var/lib/blueroute";
const STATE_DIRECTORY_ENV: &str = "BLUEROUTE_STATE_DIR";

fn main() {
    if let Err(error) = future::block_on(run()) {
        eprintln!("BlueRoute daemon failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let state_directory = std::env::var_os(STATE_DIRECTORY_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIRECTORY));
    let identity_store = NodeIdentityStore::new(node_identity_path(&state_directory));
    let local_node = identity_store.load_or_create()?;
    let local_control_tls_identity =
        ControlTlsIdentityStore::new(control_tls_identity_path(&state_directory))
            .load_or_create(local_node)?;

    let membership_path = network_membership_path(&state_directory);
    let membership_store = NetworkMembershipStore::new(membership_path.clone());
    // Fail closed instead of quietly forgetting membership/trust state when durable data is invalid.
    let membership_registry = membership_store.load()?;
    let current_network = current_network(&membership_registry)?;

    let config = DaemonConfig::default();
    let capability_report = SystemCapabilityProbe::new(config.clone())?.report().await?;
    let health = match capability_report.support {
        SystemSupportLevel::FullySupported | SystemSupportLevel::ClientOnly => HealthLevel::Healthy,
        SystemSupportLevel::Degraded => HealthLevel::Degraded,
        SystemSupportLevel::Unsupported => HealthLevel::Error,
    };
    let capabilities = capability_report.runtime.node.clone();
    let mut status = DaemonStatus {
        api_version: ApiVersion::CURRENT,
        local_node: Some(local_node),
        current_network,
        health,
        capabilities: capabilities.clone(),
    };
    // Load and validate durable cryptographic peer pins before serving requests.
    // Legacy NodeId trust alone cannot authorize a control connection, and a
    // corrupt, unsupported or insecure pin store must prevent daemon startup.
    // This does not enroll peers: the out-of-band user-verification ceremony
    // and wiring mutual TLS into the peer transport are separate mandatory
    // P7 prerequisites.
    let peer_pin_store = PeerPinStore::new(peer_pin_directory(&state_directory));
    peer_pin_store.load()?;
    let pending_join_operations = Arc::new(PendingJoinOperations::new(
        NetworkMembershipStore::new(membership_path.clone()),
        PendingJoinStore::new(pending_join_directory(&state_directory)),
    ));
    let peer_trust_operations = Arc::new(
        DurablePeerTrustOperations::with_peer_pin_store(
            NetworkMembershipStore::new(membership_path.clone()),
            peer_pin_store,
        )
        .with_pending_join(pending_join_operations.clone()),
    );
    let client_records = Arc::new(ClientRecordStore::new(client_record_directory(
        &state_directory,
    )));
    let join_network_operations = Arc::new(
        TransactionalJoinNetworkOperations::with_ipv4_pool(
            NetworkMembershipStore::new(membership_path.clone()),
            config.ipv4_address_pool,
            pending_join_operations.clone(),
            JoinControlConfig {
                // The identity file is deterministic, so loading it again yields the same key.
                local: Arc::new(
                    ControlTlsIdentityStore::new(control_tls_identity_path(&state_directory))
                        .load_or_create(local_node)?,
                ),
                trust: peer_trust_operations.clone(),
                registry: Arc::new(Mutex::new(ControlSessionRegistry::new())),
                capabilities: capabilities.clone(),
            },
        )?
        .with_client_records(client_records.clone())
        .with_authorization(Arc::new(PendingHostCommitAuthorization::new(
            pending_join_operations.clone(),
            PeerPinStore::new(peer_pin_directory(&state_directory)),
        ))),
    );
    // A persisted joined client is reconnected in the background (P6-009); the runtime shares
    // the join path's closed gate, so nothing here can touch Bluetooth while join is closed.
    let client_reconnector = join_network_operations.client_runtime().map(|runtime| {
        ClientReconnector::new(
            NetworkMembershipStore::new(membership_path.clone()),
            PeerPinStore::new(peer_pin_directory(&state_directory)),
            client_records.clone(),
            Arc::new(runtime),
            join_network_operations.supervisors(),
            config.ipv4_address_pool,
            ReconnectPolicy::default(),
        )
    });
    // A persisted joined client is never restored as a host; the background reconnector below
    // owns it.
    let joined_client = current_network
        .and_then(|network| membership_registry.network(&network))
        .is_some_and(|membership| membership.role == Some(MembershipRole::Client));
    let network_operations = Arc::new(SingleStarNetworkOperations::new(
        NetworkMembershipStore::new(membership_path),
        config.clone(),
        capabilities.clone(),
    ));
    let startup_reconciled = if joined_client {
        true
    } else if let Err(error) = network_operations.reconcile_on_startup().await {
        if status.health == HealthLevel::Healthy {
            status.health = HealthLevel::Degraded;
        }
        eprintln!(
            "{}",
            serde_json::json!({
                "component": "daemon_runtime",
                "event": "startup_reconciliation",
                "outcome": "failed",
                "failure_class": format!("{:?}", error.kind()),
            })
        );
        false
    } else {
        true
    };
    let system_diagnostics = system_diagnostics_from_report(&capability_report);
    let worker_network_operations: Arc<dyn NetworkOperations> = network_operations.clone();
    let worker_peer_trust = peer_trust_operations.clone();
    let mut service =
        DaemonService::with_operations(status, network_operations, peer_trust_operations)
            .with_join_network_operations(join_network_operations)
            .with_pending_join_operations(pending_join_operations)
            .with_system_diagnostics(system_diagnostics)
            .with_local_control_tls_fingerprint(local_control_tls_identity.fingerprint());
    if let Some(reconnector) = client_reconnector {
        service = service.with_client_reconnector(reconnector.clone());
        // After the observer is installed, so health reflects the first attempt.
        if let Err(error) = reconnector.start() {
            eprintln!(
                "{}",
                serde_json::json!({
                    "component": "daemon_runtime",
                    "event": "client_reconnect_start",
                    "outcome": "failed",
                    "failure_class": format!("{:?}", error.kind()),
                })
            );
        }
    }

    let service_for_health = service.clone();
    let _connection = Builder::system()?
        .name(DBUS_SERVICE_NAME)?
        .serve_at(DBUS_OBJECT_PATH, service)?
        .build()
        .await?;

    // Run blocking mutual-TLS I/O off the D-Bus executor. No remote session
    // authorizes network membership until production JoinNetwork is integrated.
    let _control_worker = spawn_daemon_control_worker(
        NetworkMembershipStore::new(network_membership_path(&state_directory)),
        worker_network_operations,
        worker_peer_trust,
        local_control_tls_identity,
        capabilities,
    )?;
    // Probe live BlueZ/NetworkManager availability without blocking the
    // D-Bus executor. A failed startup host restoration must not be disguised
    // as Healthy merely because services later become reachable.
    let _health_worker = spawn_daemon_capability_health_worker(
        service_for_health,
        SystemCapabilityProbe::new(config)?,
        startup_reconciled,
    )?;

    future::pending::<()>().await;
    Ok(())
}

fn node_identity_path(state_directory: &Path) -> PathBuf {
    state_directory.join("node-id")
}

fn network_membership_path(state_directory: &Path) -> PathBuf {
    state_directory.join("memberships-v1")
}

fn peer_pin_directory(state_directory: &Path) -> PathBuf {
    state_directory.join("peer-keys")
}

fn client_record_directory(state_directory: &Path) -> PathBuf {
    state_directory.join("client-record")
}

fn pending_join_directory(state_directory: &Path) -> PathBuf {
    state_directory.join("pending-join")
}

fn control_tls_identity_path(state_directory: &Path) -> PathBuf {
    state_directory.join("control-tls")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_state_paths_are_stable_inside_state_directory() {
        let state_directory = Path::new("/tmp/blueroute-state");
        assert_eq!(
            node_identity_path(state_directory),
            PathBuf::from("/tmp/blueroute-state/node-id")
        );
        assert_eq!(
            network_membership_path(state_directory),
            PathBuf::from("/tmp/blueroute-state/memberships-v1")
        );
        assert_eq!(
            peer_pin_directory(state_directory),
            PathBuf::from("/tmp/blueroute-state/peer-keys")
        );
        assert_eq!(
            control_tls_identity_path(state_directory),
            PathBuf::from("/tmp/blueroute-state/control-tls")
        );
    }
}

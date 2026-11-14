use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blueroute_core::{DaemonConfig, HealthLevel, MembershipRole};
use blueroute_daemon::{
    AuthenticatedControlPeers, CLEANUP_BOUND, ClientReconnector, ClientRecordStore,
    ControlTlsIdentityStore, DaemonService, DeviceNameStore, DurablePeerTrustOperations,
    EXPERIMENTAL_JOIN, JoinControlConfig, NetworkOperations, PeerPinStore,
    PendingHostCommitAuthorization, PendingJoinOperations, PendingJoinStore, ReconnectPolicy,
    RemovalPaths, SingleStarNetworkOperations, SystemClientLeave,
    TransactionalJoinNetworkOperations, acquire_instance_lock, cleanup_outcome, current_network,
    removal_pending, run_removal_cleanup, spawn_daemon_control_worker_with_peers,
    spawn_daemon_runtime_worker, spawn_state_publisher, system_diagnostics_from_report,
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

/// Keep the join gate closed, unless this is an `experimental-join` hardware-test build, in which
/// case open it only with everything join depends on wired, and say so loudly.
fn configure_join_gate(
    operations: TransactionalJoinNetworkOperations,
) -> Result<TransactionalJoinNetworkOperations, Box<dyn Error>> {
    if !EXPERIMENTAL_JOIN {
        return Ok(operations);
    }
    let opened = operations.open_join_gate_for_hardware_testing()?;
    eprintln!(
        "{}",
        serde_json::json!({
            "component": "daemon_runtime",
            "event": "experimental_join_enabled",
            "outcome": "warning",
            "detail": "join gate OPEN: hardware-test build; not for general use",
        })
    );
    Ok(opened)
}

/// The only argument the daemon accepts: the package-removal maintenance mode (CR-F5).
const REMOVE_OWNED_NETWORKING: &str = "--remove-owned-networking";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [] => {
            if let Err(error) = future::block_on(run()) {
                eprintln!("BlueRoute daemon failed: {error}");
                std::process::exit(1);
            }
        }
        [mode] if mode == REMOVE_OWNED_NETWORKING => {
            std::process::exit(remove_owned_networking());
        }
        _ => {
            eprintln!("usage: blueroute-daemon [{REMOVE_OWNED_NETWORKING}]");
            std::process::exit(2);
        }
    }
}

fn state_directory() -> PathBuf {
    std::env::var_os(STATE_DIRECTORY_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIRECTORY))
}

/// Package-removal cleanup (docs/CR-F5-REMOVAL-CLEANUP.md). Exit 0 on success, 1 on failure,
/// 2 when refused. Logs categories and counts only.
fn remove_owned_networking() -> i32 {
    use std::os::unix::fs::MetadataExt;
    // `/proc/self` is owned by the effective user; no `unsafe` call is needed to check for root.
    if !std::fs::metadata("/proc/self").is_ok_and(|meta| meta.uid() == 0) {
        eprintln!("blueroute-daemon {REMOVE_OWNED_NETWORKING} must run as root");
        return 2;
    }
    let state = state_directory();
    let paths = RemovalPaths {
        memberships: network_membership_path(&state),
        client_record: client_record_directory(&state),
        state_directory: state,
    };
    let outcome = future::block_on(run_removal_cleanup(
        &paths,
        || async {
            // Bluetooth is optional: NetworkManager cleanup must work while bluetoothd is down.
            match SystemClientLeave::connect().await {
                Ok(backend) => Ok((backend, true)),
                Err(_) => Ok((SystemClientLeave::connect_network_manager().await?, false)),
            }
        },
        CLEANUP_BOUND,
    ));
    let (code, line) = cleanup_outcome(&outcome);
    eprintln!("{line}");
    if code != 0 {
        eprintln!(
            "BlueRoute could not remove all of its networking state; the package stays installed. \
             Fix the cause (see the line above) and run the removal again."
        );
    }
    code
}

async fn run() -> Result<(), Box<dyn Error>> {
    let state_directory = state_directory();
    // One daemon per state directory, and none while a package removal is pending (CR-F5).
    let _instance = acquire_instance_lock(&state_directory)?;
    if removal_pending(&state_directory) {
        return Err("BlueRoute is being removed; reinstall the package to start the daemon".into());
    }
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
    let join_network_operations = Arc::new(configure_join_gate(
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
    )?);
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
    let host_check: Arc<dyn NetworkOperations> = network_operations.clone();
    let worker_peer_trust = peer_trust_operations.clone();
    let control_peers = AuthenticatedControlPeers::default();
    // A stored name wins; an unreadable file is reported and left untouched (not overwritten
    // until the user renames), and the default name is shown meanwhile.
    let device_names = Arc::new(DeviceNameStore::new(device_name_directory(
        &state_directory,
    )));
    let device_name = match device_names.load() {
        Ok(Some(name)) => name,
        Ok(None) => config.display_name.clone(),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "component": "daemon_runtime",
                    "event": "device_name_load",
                    "outcome": "failed",
                    "failure_class": format!("{:?}", error.kind()),
                })
            );
            config.display_name.clone()
        }
    };
    let mut service =
        DaemonService::with_operations(status, network_operations, peer_trust_operations)
            .with_join_network_operations(join_network_operations)
            .with_pending_join_operations(pending_join_operations)
            .with_system_diagnostics(system_diagnostics)
            .with_local_control_tls_fingerprint(local_control_tls_identity.fingerprint())
            .with_control_peers(control_peers.clone())
            .with_device_name_store(device_names, device_name);
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
    let service_for_publisher = service.clone();
    let connection = Builder::system()?
        .name(DBUS_SERVICE_NAME)?
        .serve_at(DBUS_OBJECT_PATH, service)?
        .build()
        .await?;
    // Announce every observable change to connected front ends (CR-F1).
    let _state_publisher = spawn_state_publisher(connection.clone(), service_for_publisher)?;

    // Run blocking mutual-TLS I/O off the D-Bus executor. No remote session
    // authorizes network membership until production JoinNetwork is integrated.
    let _control_worker = spawn_daemon_control_worker_with_peers(
        NetworkMembershipStore::new(network_membership_path(&state_directory)),
        worker_network_operations,
        worker_peer_trust,
        local_control_tls_identity,
        capabilities,
        control_peers,
    )?;
    // Probe live BlueZ/NetworkManager availability without blocking the
    // D-Bus executor. A failed startup host restoration must not be disguised
    // as Healthy merely because services later become reachable.
    let _health_worker = spawn_daemon_runtime_worker(
        service_for_health,
        SystemCapabilityProbe::new(config)?,
        startup_reconciled,
        Some(host_check),
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

fn device_name_directory(state_directory: &Path) -> PathBuf {
    state_directory.join("device-name")
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
        assert_eq!(
            device_name_directory(state_directory),
            PathBuf::from("/tmp/blueroute-state/device-name")
        );
    }
}

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blueroute_core::{DaemonConfig, HealthLevel};
use blueroute_daemon::{
    ControlTlsIdentityStore, DaemonService, DurablePeerTrustOperations, PeerPinStore,
    SingleStarNetworkOperations, TransactionalJoinNetworkOperations, current_network,
};
use blueroute_linux::{
    NetworkMembershipStore, NodeIdentityStore, SystemCapabilityProbe, SystemSupportLevel,
};
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus};
use blueroute_protocol::{DiagnosticAdapter, SystemDiagnosticMetadata};
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
    let _local_control_tls_identity =
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
    let capabilities = capability_report.runtime.node;
    let status = DaemonStatus {
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
    let peer_trust_operations = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
        NetworkMembershipStore::new(membership_path.clone()),
        peer_pin_store,
    ));
    let join_network_operations = Arc::new(TransactionalJoinNetworkOperations::with_ipv4_pool(
        NetworkMembershipStore::new(membership_path.clone()),
        config.ipv4_address_pool,
    )?);
    let network_operations = Arc::new(SingleStarNetworkOperations::new(
        NetworkMembershipStore::new(membership_path),
        config,
        capabilities,
    ));
    let system_diagnostics = SystemDiagnosticMetadata {
        daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: "Linux".to_owned(),
        kernel_release: capability_report.kernel_release.clone(),
        bluez_version: capability_report.bluez_version.clone(),
        network_backend_version: capability_report.network_backend_version.clone(),
        adapters: capability_report
            .controllers
            .iter()
            .map(|adapter| DiagnosticAdapter {
                id: adapter.handle.clone(),
                powered: adapter.powered,
                address: adapter.address.clone(),
                driver: adapter.driver.clone(),
            })
            .collect(),
    };
    let service = DaemonService::with_operations(status, network_operations, peer_trust_operations)
        .with_join_network_operations(join_network_operations)
        .with_system_diagnostics(system_diagnostics);

    let _connection = Builder::system()?
        .name(DBUS_SERVICE_NAME)?
        .serve_at(DBUS_OBJECT_PATH, service)?
        .build()
        .await?;

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

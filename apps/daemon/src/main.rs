use std::error::Error;
use std::path::{Path, PathBuf};

use blueroute_core::HealthLevel;
use blueroute_daemon::DaemonService;
use blueroute_linux::{
    NetworkMembershipStore, NodeIdentityStore, SystemCapabilityProbe, SystemSupportLevel,
};
use blueroute_protocol::{DBUS_OBJECT_PATH, DBUS_SERVICE_NAME};
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

    let membership_store = NetworkMembershipStore::new(network_membership_path(&state_directory));
    // Fail closed instead of quietly forgetting membership/trust state when durable data is invalid.
    let _membership_registry = membership_store.load()?;

    let capability_report = SystemCapabilityProbe::default().report().await?;
    let health = match capability_report.support {
        SystemSupportLevel::FullySupported | SystemSupportLevel::ClientOnly => HealthLevel::Healthy,
        SystemSupportLevel::Degraded => HealthLevel::Degraded,
        SystemSupportLevel::Unsupported => HealthLevel::Error,
    };
    let service = DaemonService::new(local_node, health, capability_report.runtime.node);

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
    }
}

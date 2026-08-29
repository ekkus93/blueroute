use std::path::{Path, PathBuf};

use blueroute_linux::{NetworkMembershipStore, NodeIdentityStore};

const DEFAULT_STATE_DIRECTORY: &str = "/var/lib/blueroute";
const STATE_DIRECTORY_ENV: &str = "BLUEROUTE_STATE_DIR";

fn main() {
    let state_directory = std::env::var_os(STATE_DIRECTORY_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIRECTORY));
    let identity_store = NodeIdentityStore::new(node_identity_path(&state_directory));

    if let Err(error) = identity_store.load_or_create() {
        eprintln!("BlueRoute failed to initialize its stable node identity: {error}");
        std::process::exit(1);
    }

    let membership_store = NetworkMembershipStore::new(network_membership_path(&state_directory));
    // Fail closed instead of quietly forgetting membership/trust state when durable data is invalid.
    let _membership_registry = match membership_store.load() {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("BlueRoute failed to load its network membership state: {error}");
            std::process::exit(1);
        }
    };
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

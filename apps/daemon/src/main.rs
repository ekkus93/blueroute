use std::path::{Path, PathBuf};

use blueroute_linux::NodeIdentityStore;

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
}

fn node_identity_path(state_directory: &Path) -> PathBuf {
    state_directory.join("node-id")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_identity_path_is_stable_inside_state_directory() {
        assert_eq!(
            node_identity_path(Path::new("/tmp/blueroute-state")),
            PathBuf::from("/tmp/blueroute-state/node-id")
        );
    }
}

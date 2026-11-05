//! Durable Ed25519 node identity (ADR 0001, P7-006).
//!
//! The file `node-key` holds a version line and the 32-byte private seed in hex, created with
//! owner-only permissions. The legacy random `node-id` file is never read or modified here, and
//! its value is never reused as an authenticated identity. Corrupt files are rejected, not
//! replaced, so a damaged key cannot silently rotate the node's identity.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use blueroute_core::{CoreError, ErrorKind};
use blueroute_protocol::crypto::{NodeIdentity, SEED_BYTES};

const VERSION_LINE: &str = "blueroute-node-key-v1";
const FILE_MODE: u32 = 0o600;

/// Source of the private seed. Production reads the kernel random source.
pub trait SeedSource: Send + Sync {
    fn seed(&self) -> Result<[u8; SEED_BYTES], CoreError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSeedSource;

impl SeedSource for SystemSeedSource {
    fn seed(&self) -> Result<[u8; SEED_BYTES], CoreError> {
        let mut seed = [0_u8; SEED_BYTES];
        fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut seed))
            .map_err(|e| failure("failed to read the Linux random source", e))?;
        Ok(seed)
    }
}

pub struct NodeKeyStore<S = SystemSeedSource> {
    path: PathBuf,
    source: S,
}

impl NodeKeyStore<SystemSeedSource> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_source(path, SystemSeedSource)
    }
}

impl<S: SeedSource> NodeKeyStore<S> {
    pub fn with_source(path: impl Into<PathBuf>, source: S) -> Self {
        Self {
            path: path.into(),
            source,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the persisted identity, or create it exactly once.
    pub fn load_or_create(&self) -> Result<NodeIdentity, CoreError> {
        if let Some(identity) = self.load_existing()? {
            return Ok(identity);
        }
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| failure("failed to create the node key directory", e))?;
        }
        let seed = self.source.seed()?;
        let contents = format!("{VERSION_LINE}\n{}\n", hex(&seed));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&self.path)
        {
            Ok(mut file) => {
                if let Err(e) = file
                    .write_all(contents.as_bytes())
                    .and_then(|()| file.sync_all())
                {
                    let _ = fs::remove_file(&self.path);
                    return Err(failure("failed to persist the node key", e));
                }
                Ok(NodeIdentity::from_seed(&seed))
            }
            Err(e) if e.kind() == IoErrorKind::AlreadyExists => {
                self.load_existing()?.ok_or_else(|| {
                    CoreError::new(
                        ErrorKind::PersistenceError,
                        "node key appeared during creation but could not be loaded",
                    )
                })
            }
            Err(e) => Err(failure("failed to create the node key file", e)),
        }
    }

    fn load_existing(&self) -> Result<Option<NodeIdentity>, CoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == IoErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(failure("failed to inspect the node key file", e)),
        };
        if !metadata.file_type().is_file() {
            return Err(CoreError::new(
                ErrorKind::PersistenceError,
                "node key path is not a regular file",
            ));
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(&self.path, fs::Permissions::from_mode(FILE_MODE))
                .map_err(|e| failure("failed to secure the node key file", e))?;
        }
        let text = fs::read_to_string(&self.path)
            .map_err(|e| failure("failed to read the node key file", e))?;
        let mut lines = text.lines();
        let malformed = || {
            CoreError::new(
                ErrorKind::PersistenceError,
                "node key file is malformed or has an unsupported version",
            )
        };
        if lines.next() != Some(VERSION_LINE) {
            return Err(malformed());
        }
        let seed = lines.next().and_then(unhex_seed).ok_or_else(malformed)?;
        if lines.next().is_some() {
            return Err(malformed());
        }
        Ok(Some(NodeIdentity::from_seed(&seed)))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex_seed(text: &str) -> Option<[u8; SEED_BYTES]> {
    if text.len() != SEED_BYTES * 2 || !text.is_ascii() {
        return None;
    }
    let mut out = [0_u8; SEED_BYTES];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn failure(message: &'static str, error: std::io::Error) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::PersistenceError, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "blueroute-key-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn key(&self) -> PathBuf {
            self.0.join("node-key")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Fixed(u8);

    impl SeedSource for Fixed {
        fn seed(&self) -> Result<[u8; SEED_BYTES], CoreError> {
            Ok([self.0; SEED_BYTES])
        }
    }

    #[test]
    fn first_start_creates_an_owner_only_versioned_key() {
        let dir = Dir::new();
        let store = NodeKeyStore::with_source(dir.key(), Fixed(5));
        let identity = store.load_or_create().unwrap();
        assert_eq!(identity.seed(), [5; SEED_BYTES]);
        let text = fs::read_to_string(dir.key()).unwrap();
        assert!(text.starts_with("blueroute-node-key-v1\n"));
        assert_eq!(
            fs::metadata(dir.key()).unwrap().permissions().mode() & 0o777,
            FILE_MODE
        );
    }

    #[test]
    fn restart_keeps_the_same_identity() {
        let dir = Dir::new();
        let first = NodeKeyStore::with_source(dir.key(), Fixed(1))
            .load_or_create()
            .unwrap();
        let again = NodeKeyStore::with_source(dir.key(), Fixed(2))
            .load_or_create()
            .unwrap();
        assert_eq!(first.node_id(), again.node_id());
    }

    #[test]
    fn corrupt_or_unknown_version_files_are_rejected_and_left_untouched() {
        for bad in [
            "garbage\n",
            "blueroute-node-key-v2\n00\n",
            "blueroute-node-key-v1\nzz\n",
            "blueroute-node-key-v1\n",
        ] {
            let dir = Dir::new();
            fs::write(dir.key(), bad).unwrap();
            let error = NodeKeyStore::with_source(dir.key(), Fixed(1))
                .load_or_create()
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::PersistenceError);
            assert_eq!(fs::read_to_string(dir.key()).unwrap(), bad);
        }
    }

    #[test]
    fn loose_permissions_are_tightened() {
        let dir = Dir::new();
        NodeKeyStore::with_source(dir.key(), Fixed(3))
            .load_or_create()
            .unwrap();
        fs::set_permissions(dir.key(), fs::Permissions::from_mode(0o644)).unwrap();
        NodeKeyStore::with_source(dir.key(), Fixed(4))
            .load_or_create()
            .unwrap();
        assert_eq!(
            fs::metadata(dir.key()).unwrap().permissions().mode() & 0o777,
            FILE_MODE
        );
    }

    #[test]
    fn legacy_node_id_file_is_neither_read_nor_reused() {
        let dir = Dir::new();
        let legacy = dir.0.join("node-id");
        let legacy_text = format!("{}\n", "ab".repeat(16));
        fs::write(&legacy, &legacy_text).unwrap();
        let identity = NodeKeyStore::with_source(dir.key(), Fixed(9))
            .load_or_create()
            .unwrap();
        assert_ne!(identity.node_id().to_string(), "ab".repeat(16));
        assert_eq!(fs::read_to_string(&legacy).unwrap(), legacy_text);
    }

    #[test]
    fn non_regular_path_is_rejected() {
        let dir = Dir::new();
        fs::create_dir(dir.key()).unwrap();
        assert!(
            NodeKeyStore::with_source(dir.key(), Fixed(1))
                .load_or_create()
                .is_err()
        );
    }
}

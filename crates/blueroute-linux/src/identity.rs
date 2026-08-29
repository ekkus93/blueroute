use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use blueroute_core::{CoreError, ErrorKind, NodeId};

const NODE_ID_BYTES: usize = 16;
const IDENTITY_FILE_MODE: u32 = 0o600;

/// Generates a stable-node identity when no persisted identity exists yet.
pub trait NodeIdentityGenerator: Send + Sync {
    fn generate(&self) -> Result<NodeId, CoreError>;
}

/// Generates node identities from the Linux kernel random source.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemNodeIdentityGenerator;

impl NodeIdentityGenerator for SystemNodeIdentityGenerator {
    fn generate(&self) -> Result<NodeId, CoreError> {
        let mut bytes = [0_u8; NODE_ID_BYTES];
        let mut random = File::open("/dev/urandom").map_err(|error| {
            persistence_error("failed to open the Linux random source", error)
        })?;
        random.read_exact(&mut bytes).map_err(|error| {
            persistence_error("failed to generate a stable node identity", error)
        })?;
        Ok(NodeId::from_bytes(bytes))
    }
}

/// Durable file-backed node identity storage for Linux.
///
/// The identity file contains the canonical lowercase hexadecimal `NodeId` and
/// is created with owner-only permissions. Existing malformed identity files
/// are rejected rather than silently replaced, preventing accidental identity
/// rotation after corruption.
pub struct NodeIdentityStore<G = SystemNodeIdentityGenerator> {
    path: PathBuf,
    generator: G,
}

impl NodeIdentityStore<SystemNodeIdentityGenerator> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_generator(path, SystemNodeIdentityGenerator)
    }
}

impl<G> NodeIdentityStore<G>
where
    G: NodeIdentityGenerator,
{
    pub fn with_generator(path: impl Into<PathBuf>, generator: G) -> Self {
        Self {
            path: path.into(),
            generator,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the persisted node identity, or creates it exactly once.
    pub fn load_or_create(&self) -> Result<NodeId, CoreError> {
        if let Some(identity) = self.load_existing()? {
            return Ok(identity);
        }

        self.ensure_parent_directory()?;
        let identity = self.generator.generate()?;

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(IDENTITY_FILE_MODE)
            .open(&self.path)
        {
            Ok(mut file) => {
                let serialized = format!("{identity}\n");
                if let Err(error) = file
                    .write_all(serialized.as_bytes())
                    .and_then(|()| file.sync_all())
                {
                    let _ = fs::remove_file(&self.path);
                    return Err(persistence_error(
                        "failed to persist the stable node identity",
                        error,
                    ));
                }
                Ok(identity)
            }
            Err(error) if error.kind() == IoErrorKind::AlreadyExists => self
                .load_existing()?
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorKind::PersistenceError,
                        "node identity appeared during creation but could not be loaded",
                    )
                }),
            Err(error) => Err(persistence_error(
                "failed to create the stable node identity file",
                error,
            )),
        }
    }

    fn load_existing(&self) -> Result<Option<NodeId>, CoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == IoErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(persistence_error(
                    "failed to inspect the stable node identity file",
                    error,
                ));
            }
        };

        if !metadata.file_type().is_file() {
            return Err(CoreError::new(
                ErrorKind::PersistenceError,
                "stable node identity path is not a regular file",
            ));
        }

        self.enforce_owner_only_permissions(metadata.permissions().mode())?;

        let serialized = fs::read_to_string(&self.path).map_err(|error| {
            persistence_error("failed to read the stable node identity file", error)
        })?;
        let value = serialized.strip_suffix('\n').unwrap_or(&serialized);
        let identity = value.parse::<NodeId>().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "stable node identity file is malformed",
                error.to_string(),
            )
        })?;
        Ok(Some(identity))
    }

    fn ensure_parent_directory(&self) -> Result<(), CoreError> {
        let Some(parent) = self.path.parent() else {
            return Ok(());
        };
        if parent.as_os_str().is_empty() {
            return Ok(());
        }
        fs::create_dir_all(parent).map_err(|error| {
            persistence_error("failed to create the node identity state directory", error)
        })
    }

    fn enforce_owner_only_permissions(&self, current_mode: u32) -> Result<(), CoreError> {
        if current_mode & 0o077 == 0 {
            return Ok(());
        }

        let permissions = fs::Permissions::from_mode(IDENTITY_FILE_MODE);
        fs::set_permissions(&self.path, permissions).map_err(|error| {
            persistence_error("failed to secure the stable node identity file", error)
        })
    }
}

fn persistence_error(message: &'static str, error: std::io::Error) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::PersistenceError, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_TEST_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy)]
    struct FixedGenerator(NodeId);

    impl NodeIdentityGenerator for FixedGenerator {
        fn generate(&self) -> Result<NodeId, CoreError> {
            Ok(self.0)
        }
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "blueroute-identity-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn identity_path(&self) -> PathBuf {
            self.0.join("node-id")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn id(value: u8) -> NodeId {
        NodeId::from_bytes([value; NODE_ID_BYTES])
    }

    #[test]
    fn first_start_generates_and_persists_identity() {
        let directory = TestDirectory::new();
        let path = directory.identity_path();
        let expected = id(7);
        let store = NodeIdentityStore::with_generator(&path, FixedGenerator(expected));

        assert_eq!(store.load_or_create().unwrap(), expected);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{expected}\n")
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, IDENTITY_FILE_MODE);
    }

    #[test]
    fn restart_recovers_existing_identity_instead_of_generating_another() {
        let directory = TestDirectory::new();
        let path = directory.identity_path();
        let original = id(1);
        let first = NodeIdentityStore::with_generator(&path, FixedGenerator(original));
        assert_eq!(first.load_or_create().unwrap(), original);

        let after_restart = NodeIdentityStore::with_generator(&path, FixedGenerator(id(2)));
        assert_eq!(after_restart.load_or_create().unwrap(), original);
    }

    #[test]
    fn malformed_identity_is_rejected_without_rotation() {
        let directory = TestDirectory::new();
        let path = directory.identity_path();
        fs::write(&path, "not-a-node-id\n").unwrap();

        let store = NodeIdentityStore::with_generator(&path, FixedGenerator(id(9)));
        let error = store.load_or_create().unwrap_err();

        assert_eq!(error.kind(), ErrorKind::PersistenceError);
        assert_eq!(fs::read_to_string(&path).unwrap(), "not-a-node-id\n");
    }

    #[test]
    fn existing_identity_permissions_are_tightened() {
        let directory = TestDirectory::new();
        let path = directory.identity_path();
        let expected = id(4);
        fs::write(&path, format!("{expected}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let store = NodeIdentityStore::with_generator(&path, FixedGenerator(id(5)));
        assert_eq!(store.load_or_create().unwrap(), expected);

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, IDENTITY_FILE_MODE);
    }

    #[test]
    fn non_regular_identity_path_is_rejected() {
        let directory = TestDirectory::new();
        let path = directory.identity_path();
        fs::create_dir(&path).unwrap();

        let store = NodeIdentityStore::with_generator(&path, FixedGenerator(id(3)));
        let error = store.load_or_create().unwrap_err();

        assert_eq!(error.kind(), ErrorKind::PersistenceError);
    }
}

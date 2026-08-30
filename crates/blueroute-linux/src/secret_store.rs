use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use blueroute_core::{CoreError, ErrorKind, Secret};

const SECRET_DIRECTORY_MODE: u32 = 0o700;
const SECRET_FILE_MODE: u32 = 0o600;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

/// File-backed storage for BlueRoute-owned secret material on Linux.
///
/// The store owns a dedicated directory, keeps that directory owner-only, and writes
/// each secret as an owner-only regular file using a same-directory atomic replacement.
/// Secret names must be one normal path component so callers cannot escape the store.
pub struct SecretFileStore {
    directory: PathBuf,
}

impl SecretFileStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn load(&self, name: &str) -> Result<Option<Secret<Vec<u8>>>, CoreError> {
        let path = self.secret_path(name)?;
        self.ensure_secure_directory()?;

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == IoErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(persistence_error(
                    "failed to inspect a persisted secret file",
                    error,
                ));
            }
        };

        if !metadata.file_type().is_file() {
            return Err(CoreError::new(
                ErrorKind::PersistenceError,
                "persisted secret path is not a regular file",
            ));
        }

        self.enforce_file_permissions(&path, metadata.permissions().mode())?;
        let bytes = fs::read(&path)
            .map_err(|error| persistence_error("failed to read a persisted secret", error))?;
        Ok(Some(Secret::new(bytes)))
    }

    pub fn save(&self, name: &str, secret: &Secret<Vec<u8>>) -> Result<(), CoreError> {
        let path = self.secret_path(name)?;
        self.ensure_secure_directory()?;
        self.validate_destination(&path)?;

        let temporary_path = self.temporary_path(name)?;
        let result = self.write_temporary_and_replace(&temporary_path, &path, secret.expose());
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    fn secret_path(&self, name: &str) -> Result<PathBuf, CoreError> {
        let path = Path::new(name);
        let mut components = path.components();
        let valid = matches!(components.next(), Some(Component::Normal(component)) if component == OsStr::new(name))
            && components.next().is_none();
        if !valid {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "secret name must be one normal path component",
            ));
        }
        Ok(self.directory.join(path))
    }

    fn ensure_secure_directory(&self) -> Result<(), CoreError> {
        match fs::symlink_metadata(&self.directory) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                self.enforce_directory_permissions(metadata.permissions().mode())
            }
            Ok(_) => Err(CoreError::new(
                ErrorKind::PersistenceError,
                "secret store path is not a directory",
            )),
            Err(error) if error.kind() == IoErrorKind::NotFound => {
                fs::create_dir_all(&self.directory).map_err(|error| {
                    persistence_error("failed to create the secret store directory", error)
                })?;
                let metadata = fs::symlink_metadata(&self.directory).map_err(|error| {
                    persistence_error("failed to inspect the secret store directory", error)
                })?;
                if !metadata.file_type().is_dir() {
                    return Err(CoreError::new(
                        ErrorKind::PersistenceError,
                        "secret store path is not a directory",
                    ));
                }
                self.enforce_directory_permissions(metadata.permissions().mode())
            }
            Err(error) => Err(persistence_error(
                "failed to inspect the secret store directory",
                error,
            )),
        }
    }

    fn validate_destination(&self, path: &Path) -> Result<(), CoreError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => Ok(()),
            Ok(_) => Err(CoreError::new(
                ErrorKind::PersistenceError,
                "persisted secret path is not a regular file",
            )),
            Err(error) if error.kind() == IoErrorKind::NotFound => Ok(()),
            Err(error) => Err(persistence_error(
                "failed to inspect a persisted secret file",
                error,
            )),
        }
    }

    fn temporary_path(&self, name: &str) -> Result<PathBuf, CoreError> {
        self.secret_path(name)?;
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .directory
            .join(format!(".{name}.tmp-{}-{sequence}", std::process::id())))
    }

    fn write_temporary_and_replace(
        &self,
        temporary_path: &Path,
        destination: &Path,
        secret: &[u8],
    ) -> Result<(), CoreError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(SECRET_FILE_MODE)
            .open(temporary_path)
            .map_err(|error| {
                persistence_error("failed to create a temporary secret file", error)
            })?;

        file.write_all(secret)
            .and_then(|()| file.sync_all())
            .map_err(|error| persistence_error("failed to persist secret material", error))?;
        drop(file);

        fs::rename(temporary_path, destination)
            .map_err(|error| persistence_error("failed to replace persisted secret", error))?;
        sync_directory(&self.directory)
    }

    fn enforce_directory_permissions(&self, current_mode: u32) -> Result<(), CoreError> {
        if current_mode & 0o777 == SECRET_DIRECTORY_MODE {
            return Ok(());
        }
        fs::set_permissions(
            &self.directory,
            fs::Permissions::from_mode(SECRET_DIRECTORY_MODE),
        )
        .map_err(|error| persistence_error("failed to secure the secret store directory", error))
    }

    fn enforce_file_permissions(&self, path: &Path, current_mode: u32) -> Result<(), CoreError> {
        if current_mode & 0o777 == SECRET_FILE_MODE {
            return Ok(());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(SECRET_FILE_MODE))
            .map_err(|error| persistence_error("failed to secure a persisted secret file", error))
    }
}

fn sync_directory(path: &Path) -> Result<(), CoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| persistence_error("failed to sync the secret store directory", error))
}

fn persistence_error(message: &'static str, error: std::io::Error) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::PersistenceError, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_TEST_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "blueroute-secret-store-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn secret_directory(&self) -> PathBuf {
            self.0.join("secrets")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_secret_returns_none_and_creates_private_directory() {
        let directory = TestDirectory::new();
        let secret_directory = directory.secret_directory();
        let store = SecretFileStore::new(&secret_directory);

        assert!(store.load("control-key").unwrap().is_none());
        let mode = fs::metadata(secret_directory).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, SECRET_DIRECTORY_MODE);
    }

    #[test]
    fn secret_round_trip_uses_owner_only_permissions_and_redacted_debug() {
        let directory = TestDirectory::new();
        let secret_directory = directory.secret_directory();
        let store = SecretFileStore::new(&secret_directory);
        let expected = b"super-secret-control-key".to_vec();
        let secret = Secret::new(expected.clone());

        store.save("control-key", &secret).unwrap();
        let path = secret_directory.join("control-key");
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, SECRET_FILE_MODE);

        let restored = store.load("control-key").unwrap().unwrap();
        assert_eq!(restored.expose(), &expected);
        assert!(!format!("{restored:?}").contains("super-secret-control-key"));
    }

    #[test]
    fn existing_directory_and_file_permissions_are_tightened() {
        let directory = TestDirectory::new();
        let secret_directory = directory.secret_directory();
        fs::create_dir(&secret_directory).unwrap();
        fs::set_permissions(&secret_directory, fs::Permissions::from_mode(0o755)).unwrap();
        let path = secret_directory.join("token");
        fs::write(&path, b"token-value").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let store = SecretFileStore::new(&secret_directory);
        let restored = store.load("token").unwrap().unwrap();
        assert_eq!(restored.expose().as_slice(), b"token-value");
        assert_eq!(
            fs::metadata(&secret_directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            SECRET_DIRECTORY_MODE
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            SECRET_FILE_MODE
        );
    }

    #[test]
    fn secret_names_cannot_escape_the_store_directory() {
        let directory = TestDirectory::new();
        let store = SecretFileStore::new(directory.secret_directory());
        let secret = Secret::new(b"value".to_vec());

        for invalid in ["", ".", "..", "../escape", "nested/key", "/absolute"] {
            let error = store.save(invalid, &secret).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidInput);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_secret_path_is_rejected_without_reading_target() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let secret_directory = directory.secret_directory();
        fs::create_dir(&secret_directory).unwrap();
        let target = directory.0.join("target");
        fs::write(&target, b"do-not-read").unwrap();
        symlink(&target, secret_directory.join("control-key")).unwrap();

        let error = SecretFileStore::new(secret_directory)
            .load("control-key")
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PersistenceError);
    }

    #[test]
    fn replacing_secret_is_atomic_and_leaves_no_temporary_file() {
        let directory = TestDirectory::new();
        let secret_directory = directory.secret_directory();
        let store = SecretFileStore::new(&secret_directory);
        store
            .save("control-key", &Secret::new(b"first".to_vec()))
            .unwrap();
        store
            .save("control-key", &Secret::new(b"second".to_vec()))
            .unwrap();

        let restored = store.load("control-key").unwrap().unwrap();
        assert_eq!(restored.expose().as_slice(), b"second");
        let entries = fs::read_dir(secret_directory).unwrap().count();
        assert_eq!(entries, 1);
    }
}

//! Durable P7 peer-key binding registry, stored with existing secret-file safety.
//!
//! A record represents a previously user-verified (network, NodeId, SPKI key)
//! enrollment. It does not confer current network membership or bypass TLS
//! certificate proof-of-possession. Missing/legacy state means no enrolled keys.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId, Secret};
use blueroute_linux::SecretFileStore;
use blueroute_protocol::peer_pin::{PeerKeyPins, SpkiSha256};

const FILE_NAME: &str = "peer-key-pins-v1";
const HEADER: &str = "BLUEROUTE_PEER_PINS_V1";
const MAX_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 128;
// A persistent inode, never renamed or unlinked during ordinary operation.
// The Linux O_NOFOLLOW flag prevents following a crafted lock-path symlink.
// O_NOFOLLOW is 0o400000 on supported Linux targets (asm-generic fcntl).
const PEER_LOCK_FILE: &str = ".peer-approval.lock";
const LINUX_O_NOFOLLOW: i32 = 0o400000;

/// Holds a nonblocking OS-wide advisory lock until dropped. Every BlueRoute
/// daemon handling the same persistent peer-key directory must cooperate.
/// External writers ignoring the lock are outside this guarantee.
#[derive(Debug)]
pub(crate) struct PeerPinFileLock {
    _file: File,
}

pub struct PeerPinStore {
    secrets: SecretFileStore,
}

impl PeerPinStore {
    pub fn new(secret_directory: impl Into<PathBuf>) -> Self {
        Self {
            secrets: SecretFileStore::new(secret_directory),
        }
    }

    /// Fail closed if another BlueRoute daemon instance is already reading
    /// or modifying durable peer authorization state. This lock is held
    /// across the full approval/pin read-modify-write and frame decision.
    ///
    /// Do not unlink or atomically replace the lock inode: persistent file
    /// identity is necessary for process-level advisory locking to work.
    pub(crate) fn try_lock(&self) -> Result<PeerPinFileLock, CoreError> {
        // The same existing secret-store check creates a 0700 directory and
        // rejects nonregular key-pin state before opening the lock path.
        let _ = self.secrets.load(FILE_NAME)?;
        let lock_path = self.secrets.directory().join(PEER_LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(LINUX_O_NOFOLLOW)
            .open(&lock_path)
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::PersistenceError,
                    "failed to open owner-only peer approval lock without following symlinks",
                    error.to_string(),
                )
            })?;
        let descriptor = file.metadata().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to inspect peer approval lock handle",
                error.to_string(),
            )
        })?;
        let on_disk = fs::symlink_metadata(&lock_path).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to verify persistent peer approval lock path",
                error.to_string(),
            )
        })?;
        let directory = fs::symlink_metadata(self.secrets.directory()).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to verify peer approval lock directory",
                error.to_string(),
            )
        })?;
        if !descriptor.file_type().is_file()
            || !on_disk.file_type().is_file()
            || !directory.file_type().is_dir()
            || descriptor.permissions().mode() & 0o777 != 0o600
            || directory.permissions().mode() & 0o777 != 0o700
            || descriptor.nlink() != 1
            || descriptor.ino() != on_disk.ino()
            || descriptor.dev() != on_disk.dev()
            || descriptor.uid() != directory.uid()
        {
            return Err(CoreError::new(
                ErrorKind::PersistenceError,
                "peer approval lock file or directory has unsafe identity or permissions",
            ));
        }
        match file.try_lock() {
            Ok(()) => Ok(PeerPinFileLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(CoreError::new(
                ErrorKind::InvalidState,
                "another BlueRoute daemon is updating peer approval; retry safely",
            )),
            Err(TryLockError::Error(error)) => Err(CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to acquire peer approval process lock",
                error.to_string(),
            )),
        }
    }

    pub fn load(&self) -> Result<PeerKeyPins, CoreError> {
        let Some(bytes) = self.secrets.load(FILE_NAME)? else {
            return Ok(PeerKeyPins::new());
        };
        if bytes.expose().len() > MAX_BYTES {
            return Err(invalid("peer-key pin file exceeds size limit"));
        }
        let document = std::str::from_utf8(bytes.expose())
            .map_err(|_| invalid("peer-key pin file is not valid UTF-8"))?;
        let mut lines = document.lines();
        if lines.next() != Some(HEADER) {
            return Err(invalid(
                "missing or unsupported peer-key pin format version",
            ));
        }
        let mut pins = PeerKeyPins::new();
        for line in lines {
            if pins.len() >= MAX_RECORDS {
                return Err(invalid("too many peer key pins"));
            }
            let mut fields = line.split('\t');
            let network: NetworkId = fields
                .next()
                .ok_or_else(|| invalid("missing key pin network identity"))?
                .parse()
                .map_err(|_| invalid("invalid key pin network identity"))?;
            let node: NodeId = fields
                .next()
                .ok_or_else(|| invalid("missing key pin node identity"))?
                .parse()
                .map_err(|_| invalid("invalid key pin node identity"))?;
            let fingerprint = decode_digest(
                fields
                    .next()
                    .ok_or_else(|| invalid("missing key pin fingerprint"))?,
            )?;
            if fields.next().is_some() {
                return Err(invalid("unexpected key pin record fields"));
            }
            if !pins
                .enroll_verified(network, node, fingerprint)
                .map_err(|_| invalid("conflicting duplicate key pin"))?
            {
                return Err(invalid("duplicate key pin record"));
            }
        }
        Ok(pins)
    }

    pub fn save(&self, pins: &PeerKeyPins) -> Result<(), CoreError> {
        if pins.len() > MAX_RECORDS {
            return Err(invalid("too many peer key pins"));
        }
        let mut text = String::from(HEADER);
        text.push('\n');
        for (network, node, pin) in pins.entries() {
            text.push_str(&network.to_string());
            text.push('\t');
            text.push_str(&node.to_string());
            text.push('\t');
            text.push_str(&pin.to_canonical_hex());
            text.push('\n');
        }
        if text.len() > MAX_BYTES {
            return Err(invalid("peer-key pin file exceeds size limit"));
        }
        self.secrets
            .save(FILE_NAME, &Secret::new(text.into_bytes()))
    }
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::PersistenceError, message)
}

fn decode_digest(text: &str) -> Result<SpkiSha256, CoreError> {
    SpkiSha256::parse_canonical_hex(text)
        .map_err(|_| invalid("peer key fingerprint must be canonical lowercase SHA-256 hex"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "blueroute-peer-pin-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn store(&self) -> PeerPinStore {
            PeerPinStore::new(self.0.join("secret-state"))
        }

        fn pins_file(&self) -> PathBuf {
            self.0.join("secret-state").join(FILE_NAME)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn n(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn pin(value: u8) -> SpkiSha256 {
        SpkiSha256::from_verified_certificate_digest([value; 32]).unwrap()
    }

    #[test]
    fn missing_state_means_zero_authorized_key_pins() {
        let tmp = Temp::new();
        assert!(tmp.store().load().unwrap().is_empty());
    }

    #[test]
    fn pins_round_trip_with_owner_only_permissions_and_survive_restart() {
        let tmp = Temp::new();
        let store = tmp.store();
        let mut pins = PeerKeyPins::new();
        pins.enroll_verified(n(1), node(2), pin(3)).unwrap();
        pins.enroll_verified(n(2), node(2), pin(4)).unwrap();
        store.save(&pins).unwrap();

        let file_mode = fs::metadata(tmp.pins_file()).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        let directory_mode = fs::metadata(tmp.0.join("secret-state"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);

        let loaded = tmp.store().load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.authorize(n(1), node(2), true, pin(3)).is_ok());
        assert!(loaded.authorize(n(2), node(2), true, pin(4)).is_ok());
        assert!(loaded.authorize(n(2), node(2), true, pin(3)).is_err());

        let mut revoked = loaded;
        assert!(revoked.revoke(n(1), node(2)));
        store.save(&revoked).unwrap();
        assert_eq!(tmp.store().load().unwrap().len(), 1);
    }

    #[test]
    fn corrupt_truncated_future_and_duplicate_records_fail_closed() {
        let tmp = Temp::new();
        let store = tmp.store();
        store.save(&PeerKeyPins::new()).unwrap();
        let path = tmp.pins_file();
        let good = format!("{HEADER}\n{}\t{}\t{}\n", n(1), node(2), "03".repeat(32));
        let inputs = [
            "BLUEROUTE_PEER_PINS_V2\n".to_owned(),
            format!("{HEADER}\nnot-a-record\n"),
            format!("{HEADER}\n{}\t{}\tdeadbeef\n", n(1), node(2)),
            format!("{HEADER}\n{}\t{}\t{}\n", n(1), node(2), "0G".repeat(32)),
            format!("{good}{}\t{}\t{}\n", n(1), node(2), "03".repeat(32)),
            format!("{good}{}\t{}\t{}\n", n(1), node(3), "03".repeat(32)),
            format!("{HEADER}\n{}\t{}\t{}\n", n(0), node(2), "03".repeat(32)),
            format!("{HEADER}\n{}\t{}\t{}\n", n(1), node(0), "03".repeat(32)),
            "x".repeat(MAX_BYTES + 1),
        ];
        for input in inputs {
            fs::write(&path, input.as_bytes()).unwrap();
            assert_eq!(
                store.load().unwrap_err().kind(),
                ErrorKind::PersistenceError
            );
        }
    }

    #[test]
    fn symlink_at_persistent_pin_path_is_never_followed() {
        let tmp = Temp::new();
        let store = tmp.store();
        assert!(store.load().unwrap().is_empty());
        let destination = tmp.0.join("outside");
        fs::write(&destination, b"do-not-touch").unwrap();
        symlink(&destination, tmp.pins_file()).unwrap();
        assert_eq!(
            store.load().unwrap_err().kind(),
            ErrorKind::PersistenceError
        );
        assert_eq!(
            store.save(&PeerKeyPins::new()).unwrap_err().kind(),
            ErrorKind::PersistenceError
        );
        assert_eq!(fs::read(destination).unwrap(), b"do-not-touch");
    }

    #[test]
    fn separate_handles_cannot_hold_concurrent_peer_approval_process_locks() {
        let tmp = Temp::new();
        let first = tmp.store();
        let second = tmp.store();
        let guard = first.try_lock().unwrap();
        let lock_path = tmp.0.join("secret-state").join(PEER_LOCK_FILE);
        let permissions = fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(permissions, 0o600);
        assert_eq!(
            second.try_lock().unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        drop(guard);
        assert!(second.try_lock().is_ok());
    }

    #[test]
    fn forged_symlink_and_world_readable_lock_file_fail_closed() {
        let tmp = Temp::new();
        let store = tmp.store();
        drop(store.try_lock().unwrap());
        let lock_path = tmp.0.join("secret-state").join(PEER_LOCK_FILE);
        let outside = tmp.0.join("unrelated");
        fs::write(&outside, b"do-not-touch").unwrap();
        fs::remove_file(&lock_path).unwrap();
        symlink(&outside, &lock_path).unwrap();
        assert_eq!(
            store.try_lock().unwrap_err().kind(),
            ErrorKind::PersistenceError
        );
        assert_eq!(fs::read(&outside).unwrap(), b"do-not-touch");
        fs::remove_file(&lock_path).unwrap();
        fs::write(&lock_path, b"").unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            store.try_lock().unwrap_err().kind(),
            ErrorKind::PersistenceError
        );
    }
}

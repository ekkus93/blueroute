//! Durable P7 peer-key binding registry, stored with existing secret-file safety.
//!
//! A record represents a previously user-verified (network, NodeId, SPKI key)
//! enrollment. It does not confer current network membership or bypass TLS
//! certificate proof-of-possession. Missing/legacy state means no enrolled keys.

use std::path::PathBuf;

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId, Secret};
use blueroute_linux::SecretFileStore;
use blueroute_protocol::peer_pin::{PeerKeyPins, SpkiSha256};

const FILE_NAME: &str = "peer-key-pins-v1";
const HEADER: &str = "BLUEROUTE_PEER_PINS_V1";
const MAX_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 128;

pub struct PeerPinStore {
    secrets: SecretFileStore,
}

impl PeerPinStore {
    pub fn new(secret_directory: impl Into<PathBuf>) -> Self {
        Self {
            secrets: SecretFileStore::new(secret_directory),
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
}

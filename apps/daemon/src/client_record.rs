//! Durable joined-client record for P6-009 (S2).
//!
//! Membership already holds the host `NodeId` and the pinned key. This record adds only what
//! cannot be re-derived: an optional BlueZ device path used as a **hint** to reconnect faster.
//! It is never authority: it may be stale (adapter renumbering), the peer locator is the
//! fallback, and the pinned TLS handshake authenticates whatever is reached. It is written after
//! a join commits and cleared on leave; a record for another network is ignored.

use std::path::PathBuf;

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId, Secret};
use blueroute_linux::{PeerHandle, SecretFileStore};

const FILE_NAME: &str = "client-record-v1";
const HEADER: &str = "BLUEROUTE_CLIENT_RECORD_V1";
const MAX_BYTES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientRecord {
    network: NetworkId,
    host: NodeId,
    device_hint: Option<PeerHandle>,
}

impl ClientRecord {
    pub fn new(
        network: NetworkId,
        host: NodeId,
        device_hint: Option<PeerHandle>,
    ) -> Result<Self, CoreError> {
        if network == NetworkId::from_bytes([0; 16]) || host == NodeId::from_bytes([0; 16]) {
            return Err(invalid(
                "client record requires non-zero network and host identities",
            ));
        }
        if let Some(hint) = &device_hint
            && (hint.as_str().contains(['\t', '\n', '\r']) || hint.as_str().len() > 256)
        {
            return Err(invalid(
                "client record device hint is not a plain short path",
            ));
        }
        Ok(Self {
            network,
            host,
            device_hint,
        })
    }

    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn host(&self) -> NodeId {
        self.host
    }

    /// An untrusted reconnect hint, never proof of identity.
    pub fn device_hint(&self) -> Option<&PeerHandle> {
        self.device_hint.as_ref()
    }
}

/// Owner-only durable storage for at most one client record. An empty body means no record.
pub struct ClientRecordStore {
    secrets: SecretFileStore,
}

impl ClientRecordStore {
    pub fn new(secret_directory: impl Into<PathBuf>) -> Self {
        Self {
            secrets: SecretFileStore::new(secret_directory),
        }
    }

    pub fn load(&self) -> Result<Option<ClientRecord>, CoreError> {
        let Some(bytes) = self.secrets.load(FILE_NAME)? else {
            return Ok(None);
        };
        if bytes.expose().len() > MAX_BYTES {
            return Err(invalid("client record file exceeds size limit"));
        }
        let document = std::str::from_utf8(bytes.expose())
            .map_err(|_| invalid("client record file is not valid UTF-8"))?;
        let mut lines = document.lines();
        if lines.next() != Some(HEADER) {
            return Err(invalid(
                "missing or unsupported client record format version",
            ));
        }
        let Some(record) = lines.next() else {
            return Ok(None);
        };
        if lines.next().is_some() {
            return Err(invalid("client record file holds more than one record"));
        }
        let mut fields = record.split('\t');
        let network: NetworkId = fields
            .next()
            .ok_or_else(|| invalid("missing client record network identity"))?
            .parse()
            .map_err(|_| invalid("invalid client record network identity"))?;
        let host: NodeId = fields
            .next()
            .ok_or_else(|| invalid("missing client record host identity"))?
            .parse()
            .map_err(|_| invalid("invalid client record host identity"))?;
        let hint = fields
            .next()
            .ok_or_else(|| invalid("missing client record device hint field"))?;
        if fields.next().is_some() {
            return Err(invalid("unexpected client record fields"));
        }
        let hint = if hint.is_empty() {
            None
        } else {
            Some(PeerHandle::new(hint).map_err(|_| invalid("invalid client record device hint"))?)
        };
        ClientRecord::new(network, host, hint)
            .map(Some)
            .map_err(|_| invalid("client record has a zero identity or malformed hint"))
    }

    /// The record for `network` only; a record for any other network is not this join's.
    pub fn load_for(&self, network: NetworkId) -> Result<Option<ClientRecord>, CoreError> {
        Ok(self.load()?.filter(|record| record.network == network))
    }

    pub fn save(&self, record: Option<&ClientRecord>) -> Result<(), CoreError> {
        let mut text = String::from(HEADER);
        text.push('\n');
        if let Some(record) = record {
            text.push_str(&format!(
                "{}\t{}\t{}\n",
                record.network,
                record.host,
                record.device_hint.as_ref().map_or("", PeerHandle::as_str)
            ));
        }
        self.secrets
            .save(FILE_NAME, &Secret::new(text.into_bytes()))
    }

    pub fn clear(&self) -> Result<(), CoreError> {
        self.save(None)
    }
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidState, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> NetworkId {
        NetworkId::from_bytes([1; 16])
    }

    fn host() -> NodeId {
        NodeId::from_bytes([2; 16])
    }

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (Temp, ClientRecordStore) {
        let dir = std::env::temp_dir().join(format!(
            "blueroute-client-record-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let store = ClientRecordStore::new(dir.join("client"));
        (Temp(dir), store)
    }

    fn hint() -> PeerHandle {
        PeerHandle::new("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap()
    }

    #[test]
    fn absent_record_loads_as_none() {
        let (_dir, store) = store();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn round_trips_with_and_without_a_hint() {
        let (_dir, store) = store();
        for device in [Some(hint()), None] {
            let record = ClientRecord::new(network(), host(), device).unwrap();
            store.save(Some(&record)).unwrap();
            assert_eq!(store.load().unwrap(), Some(record));
        }
    }

    #[test]
    fn clear_removes_the_record_and_is_idempotent() {
        let (_dir, store) = store();
        store
            .save(Some(
                &ClientRecord::new(network(), host(), Some(hint())).unwrap(),
            ))
            .unwrap();
        store.clear().unwrap();
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn a_record_for_another_network_is_not_returned() {
        let (_dir, store) = store();
        store
            .save(Some(
                &ClientRecord::new(network(), host(), Some(hint())).unwrap(),
            ))
            .unwrap();
        assert!(store.load_for(network()).unwrap().is_some());
        assert_eq!(
            store.load_for(NetworkId::from_bytes([9; 16])).unwrap(),
            None
        );
    }

    #[test]
    fn rejects_zero_identities_and_unsafe_hints() {
        let zero = NetworkId::from_bytes([0; 16]);
        assert!(ClientRecord::new(zero, host(), None).is_err());
        assert!(ClientRecord::new(network(), NodeId::from_bytes([0; 16]), None).is_err());
        let bad = PeerHandle::new("/org/bluez\tevil").unwrap();
        assert!(ClientRecord::new(network(), host(), Some(bad)).is_err());
    }

    #[test]
    fn rejects_corrupt_files_instead_of_guessing() {
        let (dir, store) = store();
        let record = ClientRecord::new(network(), host(), Some(hint())).unwrap();
        store.save(Some(&record)).unwrap();
        let path = dir.path().join("client").join(FILE_NAME);
        let good = std::fs::read_to_string(&path).unwrap();
        for bad in [
            good.replace(HEADER, "BLUEROUTE_CLIENT_RECORD_V0"),
            format!("{good}{good}"),
            format!("{HEADER}\nonly-one-field\n"),
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(store.load().is_err());
        }
    }
}

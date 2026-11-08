//! Joiner-side "intended host" record for P6-004 (design option A).
//!
//! The member-only approval layer in `peer_trust` authorizes nodes whose local `current_network`
//! equals the target, so a prospective joiner cannot use it. This module records, durably and
//! separately, the one host a human has chosen to join: its `NetworkId`, `NodeId` and an
//! independently confirmed control-TLS SPKI SHA-256.
//!
//! A pending-join record is **not** membership and never changes `current_network`. It exists
//! only while this node is in no network, is consulted by the outbound join dial in place of the
//! member-only checks, and is cleared on success, failure or leave. It does not replace TLS
//! proof of key possession; it only says which key the operator expects.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId, Secret};
use blueroute_linux::{NetworkMembershipStore, SecretFileStore};
use blueroute_protocol::peer_pin::SpkiSha256;

use crate::current_network;

const FILE_NAME: &str = "pending-join-v1";
const HEADER: &str = "BLUEROUTE_PENDING_JOIN_V1";
const MAX_BYTES: usize = 1024;

/// The single host the operator confirmed out of band.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingJoinHost {
    network: NetworkId,
    node: NodeId,
    fingerprint: SpkiSha256,
}

impl PendingJoinHost {
    pub fn new(
        network: NetworkId,
        node: NodeId,
        fingerprint: SpkiSha256,
    ) -> Result<Self, CoreError> {
        if network == NetworkId::from_bytes([0; 16]) || node == NodeId::from_bytes([0; 16]) {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "pending join host requires non-zero network and node identities",
            ));
        }
        Ok(Self {
            network,
            node,
            fingerprint,
        })
    }

    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }
}

/// Owner-only durable storage. An empty body means "no pending join".
pub struct PendingJoinStore {
    secrets: SecretFileStore,
}

impl PendingJoinStore {
    pub fn new(secret_directory: impl Into<PathBuf>) -> Self {
        Self {
            secrets: SecretFileStore::new(secret_directory),
        }
    }

    pub fn load(&self) -> Result<Option<PendingJoinHost>, CoreError> {
        let Some(bytes) = self.secrets.load(FILE_NAME)? else {
            return Ok(None);
        };
        if bytes.expose().len() > MAX_BYTES {
            return Err(invalid("pending join file exceeds size limit"));
        }
        let document = std::str::from_utf8(bytes.expose())
            .map_err(|_| invalid("pending join file is not valid UTF-8"))?;
        let mut lines = document.lines();
        if lines.next() != Some(HEADER) {
            return Err(invalid(
                "missing or unsupported pending join format version",
            ));
        }
        let Some(record) = lines.next() else {
            return Ok(None);
        };
        if lines.next().is_some() {
            return Err(invalid("pending join file holds more than one record"));
        }
        let mut fields = record.split('\t');
        let network: NetworkId = fields
            .next()
            .ok_or_else(|| invalid("missing pending join network identity"))?
            .parse()
            .map_err(|_| invalid("invalid pending join network identity"))?;
        let node: NodeId = fields
            .next()
            .ok_or_else(|| invalid("missing pending join node identity"))?
            .parse()
            .map_err(|_| invalid("invalid pending join node identity"))?;
        let fingerprint = SpkiSha256::parse_canonical_hex(
            fields
                .next()
                .ok_or_else(|| invalid("missing pending join fingerprint"))?,
        )
        .map_err(|_| invalid("pending join fingerprint must be canonical SHA-256 hex"))?;
        if fields.next().is_some() {
            return Err(invalid("unexpected pending join record fields"));
        }
        PendingJoinHost::new(network, node, fingerprint)
            .map(Some)
            .map_err(|_| invalid("pending join record has a zero identity"))
    }

    pub fn save(&self, host: Option<&PendingJoinHost>) -> Result<(), CoreError> {
        let mut text = String::from(HEADER);
        text.push('\n');
        if let Some(host) = host {
            text.push_str(&format!(
                "{}\t{}\t{}\n",
                host.network,
                host.node,
                host.fingerprint.to_canonical_hex()
            ));
        }
        self.secrets
            .save(FILE_NAME, &Secret::new(text.into_bytes()))
    }
}

/// Operations over the pending-join record, serialized within one daemon process.
pub struct PendingJoinOperations {
    memberships: NetworkMembershipStore,
    store: PendingJoinStore,
    lock: Mutex<()>,
}

impl PendingJoinOperations {
    pub fn new(memberships: NetworkMembershipStore, store: PendingJoinStore) -> Self {
        Self {
            memberships,
            store,
            lock: Mutex::new(()),
        }
    }

    fn guard(&self) -> Result<MutexGuard<'_, ()>, CoreError> {
        self.lock.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::Internal,
                "pending join state lock poisoned; refusing to use unverified join state",
            )
        })
    }

    fn require_not_in_a_network(&self) -> Result<(), CoreError> {
        if current_network(&self.memberships.load()?)?.is_some() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "a pending join cannot exist while this node is in a network",
            ));
        }
        Ok(())
    }

    /// Record the operator-confirmed host. Replaces an earlier record only when the caller
    /// passes the explicit replacement, never silently; one pending host at a time.
    pub fn record_host(&self, host: PendingJoinHost) -> Result<(), CoreError> {
        let _guard = self.guard()?;
        self.require_not_in_a_network()?;
        match self.store.load()? {
            Some(existing) if existing == host => Ok(()),
            Some(_) => Err(CoreError::new(
                ErrorKind::InvalidState,
                "a different pending join host is already recorded; clear it first",
            )),
            None => self.store.save(Some(&host)),
        }
    }

    pub fn pending_host(&self) -> Result<Option<PendingJoinHost>, CoreError> {
        let _guard = self.guard()?;
        self.store.load()
    }

    /// The host to join for `network`. Fails closed when none is recorded or the record names a
    /// different network, so the join can never fall back to an advertised or guessed peer.
    pub fn host_for(&self, network: NetworkId) -> Result<PendingJoinHost, CoreError> {
        let _guard = self.guard()?;
        self.require_not_in_a_network()?;
        match self.store.load()? {
            Some(host) if host.network == network => Ok(host),
            _ => Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "no operator-confirmed host is recorded for this network",
            )),
        }
    }

    /// Joiner-side counterpart of `require_approved_pinned_tls_peer`: the exact recorded host
    /// identity and key must match, and this node must still be in no network.
    pub fn require_host_pin(
        &self,
        network: NetworkId,
        node: NodeId,
        fingerprint: SpkiSha256,
    ) -> Result<(), CoreError> {
        let host = self.host_for(network)?;
        if host.node != node || host.fingerprint != fingerprint {
            return Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "peer does not match the operator-confirmed host identity and key",
            ));
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<(), CoreError> {
        let _guard = self.guard()?;
        self.store.save(None)
    }
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::PersistenceError, message)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use blueroute_core::{DisplayName, MembershipState, NetworkMembership};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "blueroute-pending-join-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn store(&self) -> PendingJoinStore {
            PendingJoinStore::new(self.0.join("secret-state"))
        }

        fn file(&self) -> PathBuf {
            self.0.join("secret-state").join(FILE_NAME)
        }

        fn operations(&self) -> PendingJoinOperations {
            PendingJoinOperations::new(
                NetworkMembershipStore::new(self.0.join("memberships")),
                self.store(),
            )
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

    fn host() -> PendingJoinHost {
        PendingJoinHost::new(n(1), node(2), pin(3)).unwrap()
    }

    #[test]
    fn missing_state_means_no_pending_join_and_no_host() {
        let tmp = Temp::new();
        let ops = tmp.operations();
        assert_eq!(ops.pending_host().unwrap(), None);
        assert_eq!(
            ops.host_for(n(1)).unwrap_err().kind(),
            ErrorKind::AuthenticationFailed
        );
    }

    #[test]
    fn zero_identities_are_rejected() {
        assert!(PendingJoinHost::new(n(0), node(2), pin(3)).is_err());
        assert!(PendingJoinHost::new(n(1), node(0), pin(3)).is_err());
    }

    #[test]
    fn record_round_trips_with_owner_only_permissions_and_survives_restart() {
        let tmp = Temp::new();
        tmp.operations().record_host(host()).unwrap();
        let mode = fs::metadata(tmp.file()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let reopened = tmp.operations();
        assert_eq!(reopened.pending_host().unwrap(), Some(host()));
        assert_eq!(reopened.host_for(n(1)).unwrap(), host());
        // Recording the identical host again is idempotent.
        reopened.record_host(host()).unwrap();
    }

    #[test]
    fn a_different_host_is_never_replaced_silently() {
        let tmp = Temp::new();
        let ops = tmp.operations();
        ops.record_host(host()).unwrap();
        let other = PendingJoinHost::new(n(1), node(9), pin(3)).unwrap();
        assert_eq!(
            ops.record_host(other).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(ops.pending_host().unwrap(), Some(host()));
        ops.clear().unwrap();
        assert_eq!(ops.pending_host().unwrap(), None);
        ops.record_host(other).unwrap();
    }

    #[test]
    fn host_pin_requires_exact_network_node_and_key() {
        let tmp = Temp::new();
        let ops = tmp.operations();
        ops.record_host(host()).unwrap();
        assert!(ops.require_host_pin(n(1), node(2), pin(3)).is_ok());
        for (network, peer, key) in [
            (n(7), node(2), pin(3)),
            (n(1), node(8), pin(3)),
            (n(1), node(2), pin(4)),
        ] {
            assert_eq!(
                ops.require_host_pin(network, peer, key).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed
            );
        }
    }

    #[test]
    fn a_node_already_in_a_network_cannot_record_or_use_a_pending_host() {
        let tmp = Temp::new();
        let ops = tmp.operations();
        ops.record_host(host()).unwrap();
        // Becoming a member (any network) must disable the joiner-side path entirely.
        let memberships = NetworkMembershipStore::new(tmp.0.join("memberships"));
        let mut registry = memberships.load().unwrap();
        let mut membership = NetworkMembership::new(n(5), DisplayName::new("Other").unwrap());
        membership.state = MembershipState::Member;
        registry.remember_network(membership);
        assert_eq!(current_network(&registry).unwrap(), Some(n(5)));
        memberships.save(&registry).unwrap();
        assert_eq!(
            ops.host_for(n(1)).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(
            ops.record_host(host()).unwrap_err().kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn corrupt_truncated_future_and_multi_record_files_fail_closed() {
        let tmp = Temp::new();
        let store = tmp.store();
        store.save(None).unwrap();
        let good = format!("{HEADER}\n{}\t{}\t{}\n", n(1), node(2), "03".repeat(32));
        let inputs = [
            "BLUEROUTE_PENDING_JOIN_V2\n".to_owned(),
            format!("{HEADER}\nnot-a-record\n"),
            format!("{HEADER}\n{}\t{}\tdeadbeef\n", n(1), node(2)),
            format!("{HEADER}\n{}\t{}\t{}\n", n(0), node(2), "03".repeat(32)),
            format!("{HEADER}\n{}\t{}\t{}\n", n(1), node(0), "03".repeat(32)),
            format!("{good}{}\t{}\t{}\n", n(2), node(3), "04".repeat(32)),
            format!("{HEADER}\n{}\t{}\t{}\tx\n", n(1), node(2), "03".repeat(32)),
            "x".repeat(MAX_BYTES + 1),
        ];
        for input in inputs {
            fs::write(tmp.file(), input.as_bytes()).unwrap();
            assert_eq!(
                store.load().unwrap_err().kind(),
                ErrorKind::PersistenceError,
                "{input:?}"
            );
        }
    }

    #[test]
    fn symlink_at_the_record_path_is_never_followed() {
        let tmp = Temp::new();
        let store = tmp.store();
        assert_eq!(store.load().unwrap(), None);
        let outside = tmp.0.join("outside");
        fs::write(&outside, b"do-not-touch").unwrap();
        symlink(&outside, tmp.file()).unwrap();
        assert_eq!(
            store.load().unwrap_err().kind(),
            ErrorKind::PersistenceError
        );
        assert_eq!(
            store.save(Some(&host())).unwrap_err().kind(),
            ErrorKind::PersistenceError
        );
        assert_eq!(fs::read(outside).unwrap(), b"do-not-touch");
    }
}

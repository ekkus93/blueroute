//! Durable authorization step of the P6-004 join commit (design option A).
//!
//! While joining, only the operator-confirmed pending host is authorized (see `pending_join`).
//! Once `Member` is committed that joiner-side fallback stops applying, so the host must already
//! be trusted and its key pinned in the member stores, or supervision would drop the session
//! immediately. This module performs that step before membership is saved and can undo it.
//!
//! Order inside [`JoinCommitAuthorization::prepare_commit`]:
//! 1. mark the host trusted in the (not yet saved) membership registry;
//! 2. enroll the host key in the durable pin store;
//!
//! The pending record is cleared by [`JoinCommitAuthorization::finalize_commit`] only **after**
//! membership is saved. Clearing it earlier would leave a window in which the live control
//! session is authorized by neither the joiner fallback nor the member stores, and per-second
//! supervision could drop it at the moment of commit. If finalizing fails, the coordinator
//! restores the previous membership and calls `undo_commit`, so a stale record can never survive
//! a completed join.
//!
//! Step 1 is in memory only; step 2 is undone on failure, never half-applied.

use std::sync::Arc;

use blueroute_core::{CoreError, ErrorKind, MembershipRegistry, NetworkId, NodeId};

use crate::{PeerPinStore, PendingJoinOperations};

/// What `prepare_commit` durably changed, so a failed membership save can be undone exactly.
#[derive(Debug, Eq, PartialEq)]
pub struct JoinCommitReceipt {
    network: NetworkId,
    node: NodeId,
    pin_added: bool,
}

impl JoinCommitReceipt {
    /// The host whose key this commit trusted.
    pub const fn host(&self) -> NodeId {
        self.node
    }
}

pub trait JoinCommitAuthorization: Send + Sync {
    /// Trust the confirmed host in `registry` (in memory) and make the durable pin and pending
    /// record changes. On error nothing durable remains changed.
    fn prepare_commit(
        &self,
        network: NetworkId,
        registry: &mut MembershipRegistry,
    ) -> Result<JoinCommitReceipt, CoreError>;

    /// Called after membership was saved: forget the operator's pending host so stale intent
    /// cannot authorize a later attempt. On error the caller reverts the commit.
    fn finalize_commit(&self, receipt: &JoinCommitReceipt) -> Result<(), CoreError>;

    /// Reverse the durable pin written by `prepare_commit` after the membership save failed.
    fn undo_commit(&self, receipt: &JoinCommitReceipt) -> Result<(), CoreError>;

    /// Forget the operator's pending host after a join attempt that mutated the runtime failed.
    fn discard(&self, network: NetworkId) -> Result<(), CoreError>;
}

pub struct PendingHostCommitAuthorization {
    pending: Arc<PendingJoinOperations>,
    pins: PeerPinStore,
}

impl PendingHostCommitAuthorization {
    pub fn new(pending: Arc<PendingJoinOperations>, pins: PeerPinStore) -> Self {
        Self { pending, pins }
    }
}

impl JoinCommitAuthorization for PendingHostCommitAuthorization {
    fn prepare_commit(
        &self,
        network: NetworkId,
        registry: &mut MembershipRegistry,
    ) -> Result<JoinCommitReceipt, CoreError> {
        let host = self.pending.host_for(network)?;
        let membership = registry.network_mut(&network).ok_or_else(|| {
            CoreError::new(
                ErrorKind::InvalidState,
                "joined network is missing from the membership registry being committed",
            )
        })?;
        membership.trust_peer(host.node());

        let _lock = self.pins.try_lock()?;
        let mut pins = self.pins.load()?;
        let pin_added = pins
            .enroll_verified(network, host.node(), host.fingerprint())
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::AuthenticationFailed,
                    "the confirmed host key conflicts with an existing enrolled key",
                    error.to_string(),
                )
            })?;
        if pin_added {
            self.pins.save(&pins)?;
        }
        Ok(JoinCommitReceipt {
            network,
            node: host.node(),
            pin_added,
        })
    }

    fn finalize_commit(&self, _receipt: &JoinCommitReceipt) -> Result<(), CoreError> {
        self.pending.clear()
    }

    fn undo_commit(&self, receipt: &JoinCommitReceipt) -> Result<(), CoreError> {
        let _lock = self.pins.try_lock()?;
        revoke_added_pin(&self.pins, receipt)
    }

    fn discard(&self, _network: NetworkId) -> Result<(), CoreError> {
        self.pending.clear()
    }
}

/// Caller must hold the pin-store process lock. Only a pin this join added is removed; a
/// pre-existing identical pin is never revoked.
fn revoke_added_pin(pins: &PeerPinStore, receipt: &JoinCommitReceipt) -> Result<(), CoreError> {
    if !receipt.pin_added {
        return Ok(());
    }
    let mut current = pins.load()?;
    if current.revoke(receipt.network, receipt.node) {
        pins.save(&current)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use blueroute_core::{DisplayName, MembershipState, NetworkMembership};
    use blueroute_linux::NetworkMembershipStore;
    use blueroute_protocol::peer_pin::SpkiSha256;

    use super::*;
    use crate::{PendingJoinHost, PendingJoinStore};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        pending: Arc<PendingJoinOperations>,
        auth: PendingHostCommitAuthorization,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn n() -> NetworkId {
        NetworkId::from_bytes([1; 16])
    }

    fn host_node() -> NodeId {
        NodeId::from_bytes([2; 16])
    }

    fn pin(value: u8) -> SpkiSha256 {
        SpkiSha256::from_verified_certificate_digest([value; 32]).unwrap()
    }

    fn fixture() -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "blueroute-join-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let pending = Arc::new(PendingJoinOperations::new(
            NetworkMembershipStore::new(root.join("memberships")),
            PendingJoinStore::new(root.join("pending")),
        ));
        let auth = PendingHostCommitAuthorization::new(
            pending.clone(),
            PeerPinStore::new(root.join("pins")),
        );
        Fixture {
            root,
            pending,
            auth,
        }
    }

    fn joined_registry() -> MembershipRegistry {
        let mut membership = NetworkMembership::new(n(), DisplayName::new("Joined").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        registry
    }

    fn record(f: &Fixture, key: u8) {
        f.pending
            .record_host(PendingJoinHost::new(n(), host_node(), pin(key)).unwrap())
            .unwrap();
    }

    #[test]
    fn commit_trusts_and_pins_the_host_then_finalize_clears_the_pending_record() {
        let f = fixture();
        record(&f, 7);
        let mut registry = joined_registry();
        let receipt = f.auth.prepare_commit(n(), &mut registry).unwrap();
        assert!(receipt.pin_added);
        assert!(
            registry
                .network(&n())
                .unwrap()
                .is_peer_trusted(&host_node())
        );
        let pins = PeerPinStore::new(f.root.join("pins")).load().unwrap();
        assert!(pins.authorize(n(), host_node(), true, pin(7)).is_ok());
        // Not cleared yet: the live session must stay authorized until membership is saved.
        assert!(f.pending.pending_host().unwrap().is_some());
        f.auth.finalize_commit(&receipt).unwrap();
        assert_eq!(f.pending.pending_host().unwrap(), None);
    }

    #[test]
    fn without_a_recorded_host_nothing_is_trusted_or_pinned() {
        let f = fixture();
        let mut registry = joined_registry();
        assert!(f.auth.prepare_commit(n(), &mut registry).is_err());
        assert!(
            !registry
                .network(&n())
                .unwrap()
                .is_peer_trusted(&host_node())
        );
        assert!(
            PeerPinStore::new(f.root.join("pins"))
                .load()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_conflicting_existing_pin_blocks_the_commit_and_keeps_the_record() {
        let f = fixture();
        let store = PeerPinStore::new(f.root.join("pins"));
        let mut pins = store.load().unwrap();
        pins.enroll_verified(n(), host_node(), pin(9)).unwrap();
        store.save(&pins).unwrap();
        record(&f, 7);
        let mut registry = joined_registry();
        let error = f.auth.prepare_commit(n(), &mut registry).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
        assert!(
            store
                .load()
                .unwrap()
                .authorize(n(), host_node(), true, pin(9))
                .is_ok()
        );
        assert!(f.pending.pending_host().unwrap().is_some());
    }

    #[test]
    fn undo_removes_only_a_pin_this_commit_added() {
        let f = fixture();
        record(&f, 7);
        let mut registry = joined_registry();
        let receipt = f.auth.prepare_commit(n(), &mut registry).unwrap();
        f.auth.undo_commit(&receipt).unwrap();
        assert!(
            PeerPinStore::new(f.root.join("pins"))
                .load()
                .unwrap()
                .is_empty()
        );

        // A pre-existing identical pin is not ours to revoke.
        let store = PeerPinStore::new(f.root.join("pins"));
        let mut pins = store.load().unwrap();
        pins.enroll_verified(n(), host_node(), pin(7)).unwrap();
        store.save(&pins).unwrap();
        record(&f, 7);
        let mut registry = joined_registry();
        let receipt = f.auth.prepare_commit(n(), &mut registry).unwrap();
        assert!(!receipt.pin_added);
        f.auth.undo_commit(&receipt).unwrap();
        assert_eq!(store.load().unwrap().len(), 1);
    }

    #[test]
    fn discard_forgets_the_operator_intent() {
        let f = fixture();
        record(&f, 7);
        f.auth.discard(n()).unwrap();
        assert_eq!(f.pending.pending_host().unwrap(), None);
    }
}

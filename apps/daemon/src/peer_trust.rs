use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId};
use blueroute_linux::{BluetoothBackend, BluezBackend, NetworkMembershipStore, PeerHandle};
use blueroute_protocol::peer_pin::SpkiSha256;

use crate::peer_pin_store::PeerPinFileLock;
use crate::{PeerPinStore, PendingJoinOperations, PinnedControlTlsPeer, current_network};

pub type PeerTrustFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

pub trait PeerTrustOperations: Send + Sync {
    fn trust_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()>;
    fn forget_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()>;

    /// Trust `node` in the current network and pin the key the operator typed from that peer's
    /// own `peer local-key`. `Ok(false)` when exactly that pin already existed.
    fn enroll_peer_key(&self, node: NodeId, fingerprint: SpkiSha256) -> PeerTrustFuture<'_, bool> {
        let _ = (node, fingerprint);
        Box::pin(async {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "peer key enrollment is not available in this daemon instance",
            ))
        })
    }
}

pub struct DurablePeerTrustOperations {
    store: NetworkMembershipStore,
    pin_store: Option<PeerPinStore>,
    // Joiner-side fallback (P6-004 option A), used only while this node is in no network.
    pending_join: Option<Arc<PendingJoinOperations>>,
    // Serialize read/modify/write state transitions within the one D-Bus
    // daemon. This does not protect against unauthorized external file edits
    // or independently running daemon processes.
    state_lock: Mutex<()>,
}

/// Both RAII guards live through a complete trust/pin transaction.
/// This is advisory cooperation between BlueRoute instances, not a defense
/// against arbitrary privileged file tampering or actual TLS authentication.
struct PeerTrustStateGuard<'a> {
    // Drop the process lock before the thread mutex, otherwise the next
    // local worker may acquire the mutex while the previous OS lock is held.
    _process_lock: Option<PeerPinFileLock>,
    _thread_guard: MutexGuard<'a, ()>,
}

impl DurablePeerTrustOperations {
    pub fn new(store: NetworkMembershipStore) -> Self {
        Self {
            store,
            pin_store: None,
            pending_join: None,
            state_lock: Mutex::new(()),
        }
    }

    /// Enable durable key enrollment lookups. Legacy trust-only construction
    /// remains valid for P6-003, but cannot grant cryptographic P7 access.
    pub fn with_peer_pin_store(store: NetworkMembershipStore, pins: PeerPinStore) -> Self {
        Self {
            store,
            pin_store: Some(pins),
            pending_join: None,
            state_lock: Mutex::new(()),
        }
    }

    /// Let a node that is in **no** network authorize exactly the operator-confirmed pending
    /// join host (network, node and key) for pinned TLS. Every member-side decision, including
    /// approval lists for inbound sessions, keeps its original member-only behavior; the
    /// fallback never applies once this node has a current network.
    pub fn with_pending_join(mut self, pending_join: Arc<PendingJoinOperations>) -> Self {
        self.pending_join = Some(pending_join);
        self
    }

    fn lock_state(&self) -> Result<PeerTrustStateGuard<'_>, CoreError> {
        let thread_guard = self.state_lock.lock().map_err(|_| {
            CoreError::new(
                ErrorKind::Internal,
                "peer trust state lock poisoned; refusing to use unverified approval state",
            )
        })?;
        // The local mutex MUST be taken before the shared on-disk lock so
        // operations cannot invert lock order in a single daemon process.
        let process_lock = self
            .pin_store
            .as_ref()
            .map(PeerPinStore::try_lock)
            .transpose()?;
        Ok(PeerTrustStateGuard {
            _thread_guard: thread_guard,
            _process_lock: process_lock,
        })
    }

    pub fn require_peer_approved(&self, network: NetworkId, node: NodeId) -> Result<(), CoreError> {
        require_nonzero_peer_identity(node)?;
        let registry = self.store.load()?;
        let current = current_network(&registry)?.ok_or_else(no_current_network)?;
        if current != network {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "refusing peer authorization for a network other than the current network",
            ));
        }
        let membership = registry.network(&network).ok_or_else(|| {
            CoreError::new(
                ErrorKind::InvalidState,
                "current BlueRoute network is missing from durable membership state",
            )
        })?;
        if membership.is_peer_trusted(&node) {
            Ok(())
        } else {
            Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "BlueRoute peer has not been explicitly approved for this network",
            ))
        }
    }

    /// Read the inbound TLS allowlist directly from current, durable
    /// P6-003 peer approvals intersected with independently enrolled SPKI
    /// pins. A historical pin without current approval is never admitted.
    /// This snapshot is only a pre-handshake filter: successful TLS activation
    /// and every frame still recheck current approval.
    pub fn approved_control_pins(
        &self,
        network: NetworkId,
    ) -> Result<Vec<PinnedControlTlsPeer>, CoreError> {
        let _guard = self.lock_state()?;
        let registry = self.store.load()?;
        if current_network(&registry)? != Some(network) {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "requested TLS pin snapshot does not match the active network",
            ));
        }
        let membership = registry.network(&network).ok_or_else(|| {
            CoreError::new(
                ErrorKind::InvalidState,
                "active network is missing from durable member registry",
            )
        })?;
        let pin_store = self.pin_store.as_ref().ok_or_else(|| {
            CoreError::new(
                ErrorKind::AuthenticationFailed,
                "no enrolled TLS peer key store is configured",
            )
        })?;
        let pins = pin_store.load()?;
        let mut approved = Vec::new();
        for (pin_network, node, fingerprint) in pins.entries() {
            if pin_network != network || !membership.is_peer_trusted(&node) {
                continue;
            }
            let pinned = PinnedControlTlsPeer::new(network, node, fingerprint).map_err(|_| {
                CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "invalid enrolled TLS peer key in durable snapshot",
                )
            })?;
            approved.push(pinned);
        }
        Ok(approved)
    }

    /// P7 key-binding gate over the existing durable P6-003 approval policy.
    ///
    /// The TLS layer must first validate the presented certificate and proof
    /// of private-key possession, then provide its verified SPKI-SHA256. A
    /// NodeId-only trust entry is insufficient: a separate explicit key pin
    /// must also exist for precisely this (NetworkId, NodeId).
    ///
    /// The verified pin is reloaded from protected storage on each decision;
    /// an old in-memory key cache must not resurrect revoked enrollment.
    /// This is a policy integration only: there is no TLS handshake or
    /// production JoinNetwork activation yet.
    pub fn require_approved_pinned_tls_peer(
        &self,
        network: NetworkId,
        node: NodeId,
        verified_spki_sha256: SpkiSha256,
    ) -> Result<(), CoreError> {
        self.with_approved_pinned_tls_peer(network, node, verified_spki_sha256, || ())
    }

    /// Serialize a trust/pin check and one local control-frame operation
    /// against in-process trust updates and revocations. The caller MUST
    /// supply a cryptographically verified TLS SPKI, not a peer claim.
    ///
    /// The state lock stays held through `operation`: a concurrent
    /// `forget_peer` cannot revoke approval in between the check and frame
    /// delivery. The operation must not block on network I/O or reenter
    /// peer-trust operations. This does not protect against external writers
    /// or authenticate a TLS certificate by itself.
    pub fn with_approved_pinned_tls_peer<T>(
        &self,
        network: NetworkId,
        node: NodeId,
        verified_spki_sha256: SpkiSha256,
        operation: impl FnOnce() -> T,
    ) -> Result<T, CoreError> {
        let _guard = self.lock_state()?;
        if let Some(pending) = &self.pending_join
            && current_network(&self.store.load()?)?.is_none()
        {
            // Joiner: only the recorded host and its confirmed key are acceptable. The TLS
            // layer has already proven possession of that key before this is consulted.
            pending.require_host_pin(network, node, verified_spki_sha256)?;
            return Ok(operation());
        }
        self.require_peer_approved(network, node)?;
        let store = self.pin_store.as_ref().ok_or_else(|| {
            CoreError::new(
                ErrorKind::AuthenticationFailed,
                "no durable peer key pin store is configured for authenticated control",
            )
        })?;
        let pins = store.load()?;
        pins.authorize(network, node, true, verified_spki_sha256)
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::AuthenticationFailed,
                    "peer has no matching explicitly enrolled BlueRoute cryptographic identity",
                    error.to_string(),
                )
            })?;
        Ok(operation())
    }

    /// Persist a key enrollment ONLY after an independently authenticated
    /// human has compared the remote peer's key fingerprint out of band.
    ///
    /// The opaque peer identity can only be created by the mutual TLS verifier.
    /// `confirmed_spki` MUST be independently verified by the operator. This
    /// helper is not itself an enrollment ceremony; a PolicyKit-protected UI
    /// must collect that confirmation before calling it.
    pub fn enroll_key_after_independent_confirmation(
        &self,
        verified_peer: &crate::VerifiedControlTlsPeer,
        confirmed_spki: SpkiSha256,
    ) -> Result<bool, CoreError> {
        self.enroll_key_inner(
            verified_peer.network(),
            verified_peer.node(),
            verified_peer.fingerprint(),
            confirmed_spki,
            None,
        )
    }

    /// Enforce the confirmation deadline under the durable trust/pin lock,
    /// including after any wait for an in-process enrollment transaction.
    /// Expired confirmation never authorizes a fresh persistent key pin.
    pub fn enroll_key_after_independent_confirmation_until(
        &self,
        verified_peer: &crate::VerifiedControlTlsPeer,
        confirmed_spki: SpkiSha256,
        deadline: Instant,
    ) -> Result<bool, CoreError> {
        self.enroll_key_inner(
            verified_peer.network(),
            verified_peer.node(),
            verified_peer.fingerprint(),
            confirmed_spki,
            Some(deadline),
        )
    }

    /// Synthetic policy seam for tests and internal callers. Production enrollment uses either
    /// the opaque TLS-verified peer entry point above or `enroll_peer_key`, where an operator
    /// types a key obtained out of band before first contact (P7-003, maintainer decision
    /// 2026-11-12); TLS then proves possession of that exact key on every connection.
    #[cfg(test)]
    pub(crate) fn enroll_verified_key(
        &self,
        network: NetworkId,
        node: NodeId,
        verified_tls_spki: SpkiSha256,
        confirmed_spki: SpkiSha256,
    ) -> Result<bool, CoreError> {
        self.enroll_key_inner(network, node, verified_tls_spki, confirmed_spki, None)
    }

    fn enroll_key_inner(
        &self,
        network: NetworkId,
        node: NodeId,
        verified_tls_spki: SpkiSha256,
        confirmed_spki: SpkiSha256,
        deadline: Option<Instant>,
    ) -> Result<bool, CoreError> {
        let _guard = self.lock_state()?;
        self.require_peer_approved(network, node)?;
        if verified_tls_spki != confirmed_spki {
            return Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "independently confirmed peer key does not match verified TLS identity",
            ));
        }
        let store = self.pin_store.as_ref().ok_or_else(|| {
            CoreError::new(
                ErrorKind::AuthenticationFailed,
                "no durable peer key pin store is configured for enrollment",
            )
        })?;
        let mut pins = store.load()?;
        // The outer transport check can expire while this operation waits on
        // a concurrent trust/pin update. Recheck after locking and disk reads.
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err(CoreError::new(
                ErrorKind::AuthenticationFailed,
                "confirmed enrollment deadline expired before durable key enrollment",
            ));
        }
        let changed = pins
            .enroll_verified(network, node, verified_tls_spki)
            .map_err(|_| {
                CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "peer identity or key conflicts with an existing enrollment",
                )
            })?;
        if changed {
            // Do not begin a persistent write using an expired confirmation.
            if deadline.is_some_and(|limit| Instant::now() >= limit) {
                return Err(CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "confirmed enrollment deadline expired before durable key enrollment",
                ));
            }
            store.save(&pins)?;
        }
        Ok(changed)
    }
}

impl PeerTrustOperations for DurablePeerTrustOperations {
    fn enroll_peer_key(&self, node: NodeId, fingerprint: SpkiSha256) -> PeerTrustFuture<'_, bool> {
        Box::pin(async move {
            require_nonzero_peer_identity(node)?;
            let network = current_network(&self.store.load()?)?.ok_or_else(no_current_network)?;
            // Trust first: the pin requires current approval. Trust alone grants no TLS access,
            // so a refused pin (a different key already enrolled) leaves nothing usable.
            self.trust_peer(node).await?;
            // The operator-typed key stands in for both inputs: there is no connection yet, and
            // every later connection must prove possession of exactly this key.
            self.enroll_key_inner(network, node, fingerprint, fingerprint, None)
        })
    }

    fn trust_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()> {
        Box::pin(async move {
            require_nonzero_peer_identity(node)?;
            let _guard = self.lock_state()?;
            let mut registry = self.store.load()?;
            let network = current_network(&registry)?.ok_or_else(no_current_network)?;
            let membership = registry.network_mut(&network).ok_or_else(|| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "current BlueRoute network is missing from durable membership state",
                )
            })?;
            if membership.trust_peer(node) {
                self.store.save(&registry)?;
            }
            Ok(())
        })
    }

    fn forget_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()> {
        Box::pin(async move {
            require_nonzero_peer_identity(node)?;
            let _guard = self.lock_state()?;
            let mut registry = self.store.load()?;
            let network = current_network(&registry)?.ok_or_else(no_current_network)?;
            let membership = registry.network_mut(&network).ok_or_else(|| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "current BlueRoute network is missing from durable membership state",
                )
            })?;
            // Remove the durable cryptographic pin FIRST, so a failure or
            // crash during the following membership write cannot leave a
            // trusted old key after a completed revocation attempt.
            if let Some(pin_store) = &self.pin_store {
                let mut pins = pin_store.load()?;
                if pins.revoke(network, node) {
                    pin_store.save(&pins)?;
                }
            }
            if membership.forget_peer(&node) {
                self.store.save(&registry)?;
            }
            Ok(())
        })
    }
}

/// Establishes Bluetooth transport trust for a BlueZ peer selected by the caller.
///
/// This does not grant BlueRoute membership and must not be used to derive a `NodeId` from a
/// Bluetooth address, object path, or display name.
pub async fn pair_and_trust_bluetooth_peer(peer: PeerHandle) -> Result<(), CoreError> {
    let bluez = BluezBackend::connect_system().await?;
    bluez.pair(peer.clone()).await?;
    bluez.set_trusted(peer, true).await
}

/// A zero NodeId is an invalid sentinel, not a peer that may receive durable
/// trust or be used as an authorization target. Reject it before state I/O.
fn require_nonzero_peer_identity(node: NodeId) -> Result<(), CoreError> {
    if node == NodeId::from_bytes([0; 16]) {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "all-zero peer NodeId cannot be trusted or authorized",
        ));
    }
    Ok(())
}

fn no_current_network() -> CoreError {
    CoreError::new(
        ErrorKind::InvalidState,
        "this daemon does not currently belong to a BlueRoute network",
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use blueroute_core::{DisplayName, MembershipRegistry, MembershipState, NetworkMembership};
    use blueroute_protocol::peer_pin::PeerKeyPins;

    use super::*;

    static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

    fn temp_store(label: &str) -> (std::path::PathBuf, NetworkMembershipStore) {
        let root = std::env::temp_dir().join(format!(
            "blueroute-p6-003-{label}-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        (
            root.clone(),
            NetworkMembershipStore::new(root.join("memberships-v1")),
        )
    }

    fn seed_current_network(store: &NetworkMembershipStore, network: NetworkId) {
        let mut membership =
            NetworkMembership::new(network, DisplayName::new("Approval test").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        store.save(&registry).unwrap();
    }

    #[test]
    fn approval_persists_without_marking_peer_member() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("approve");
            let network = NetworkId::from_bytes([0x31; 16]);
            let peer = NodeId::from_bytes([0x42; 16]);
            seed_current_network(&store, network);
            let operations = DurablePeerTrustOperations::new(store);

            operations.trust_peer(peer).await.unwrap();
            operations.require_peer_approved(network, peer).unwrap();

            let registry = operations.store.load().unwrap();
            let membership = registry.network(&network).unwrap();
            assert!(membership.is_peer_trusted(&peer));
            assert!(!membership.is_peer_member(&peer));
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn zero_peer_identity_is_rejected_before_any_durable_trust_mutation() {
        let (root, store) = temp_store("zero-peer-identity");
        let network = NetworkId::from_bytes([0x51; 16]);
        let zero = NodeId::from_bytes([0; 16]);
        let valid = NodeId::from_bytes([0x52; 16]);
        seed_current_network(&store, network);
        let trust = DurablePeerTrustOperations::with_peer_pin_store(
            store,
            PeerPinStore::new(root.join("peer-keys")),
        );
        for result in [
            futures_lite::future::block_on(trust.trust_peer(zero)),
            futures_lite::future::block_on(trust.forget_peer(zero)),
            trust.require_peer_approved(network, zero),
        ] {
            assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidInput);
        }
        let registry = trust.store.load().unwrap();
        let membership = registry.network(&network).unwrap();
        assert!(!membership.is_peer_trusted(&zero));
        assert!(trust.pin_store.as_ref().unwrap().load().unwrap().is_empty());
        // Rejecting the sentinel must not poison ordinary peer approval.
        futures_lite::future::block_on(trust.trust_peer(valid)).unwrap();
        trust.require_peer_approved(network, valid).unwrap();
        futures_lite::future::block_on(trust.forget_peer(valid)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unapproved_peer_fails_closed_and_forget_revokes_approval() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("revoke");
            let network = NetworkId::from_bytes([0x51; 16]);
            let peer = NodeId::from_bytes([0x62; 16]);
            seed_current_network(&store, network);
            let operations = DurablePeerTrustOperations::new(store);

            let error = operations.require_peer_approved(network, peer).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);

            operations.trust_peer(peer).await.unwrap();
            operations.forget_peer(peer).await.unwrap();
            let error = operations.require_peer_approved(network, peer).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn key_pin_is_required_in_addition_to_durable_node_approval() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("p7-key-enrollment");
            let network = NetworkId::from_bytes([0x11; 16]);
            let node = NodeId::from_bytes([0x22; 16]);
            let fingerprint = SpkiSha256::from_verified_certificate_digest([0x33; 32]).unwrap();
            let wrong_fingerprint =
                SpkiSha256::from_verified_certificate_digest([0x44; 32]).unwrap();
            seed_current_network(&store, network);
            let pin_store = PeerPinStore::new(root.join("peer-keys"));
            let trust = DurablePeerTrustOperations::with_peer_pin_store(store, pin_store);
            trust.trust_peer(node).await.unwrap();

            let check =
                |fingerprint| trust.require_approved_pinned_tls_peer(network, node, fingerprint);
            assert_eq!(
                check(fingerprint).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed,
            );
            let mut pins = PeerKeyPins::new();
            pins.enroll_verified(network, node, fingerprint).unwrap();
            trust.pin_store.as_ref().unwrap().save(&pins).unwrap();
            assert_eq!(
                check(wrong_fingerprint).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed,
            );
            check(fingerprint).unwrap();

            // Revoke clears the persistent pin before durable NodeId approval.
            trust.forget_peer(node).await.unwrap();
            assert!(trust.pin_store.as_ref().unwrap().load().unwrap().is_empty());
            assert_eq!(
                check(fingerprint).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed,
            );

            // Reapproving the old logical ID alone MUST NOT resurrect its key.
            trust.trust_peer(node).await.unwrap();
            assert_eq!(
                check(fingerprint).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed,
            );
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn operator_typed_enrollment_trusts_and_pins_exactly_that_key() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("p7-typed-enrollment");
            let network = NetworkId::from_bytes([0x61; 16]);
            let node = NodeId::from_bytes([0x62; 16]);
            let key = SpkiSha256::from_verified_certificate_digest([0x63; 32]).unwrap();
            let other_key = SpkiSha256::from_verified_certificate_digest([0x64; 32]).unwrap();

            // No current network: nothing to enroll into, and nothing is written.
            let pin_store = PeerPinStore::new(root.join("peer-keys"));
            let trust = DurablePeerTrustOperations::with_peer_pin_store(
                NetworkMembershipStore::new(root.join("memberships-v1")),
                pin_store,
            );
            assert_eq!(
                trust.enroll_peer_key(node, key).await.unwrap_err().kind(),
                ErrorKind::InvalidState
            );
            assert!(trust.pin_store.as_ref().unwrap().load().unwrap().is_empty());

            seed_current_network(&store, network);
            let check = |key| trust.require_approved_pinned_tls_peer(network, node, key);
            assert!(check(key).is_err());
            // One step trusts the peer and pins the typed key; the listener check now passes
            // for exactly that key and no other.
            assert!(trust.enroll_peer_key(node, key).await.unwrap());
            check(key).unwrap();
            assert_eq!(
                check(other_key).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed
            );
            assert_eq!(trust.approved_control_pins(network).unwrap().len(), 1);
            // Repeating it is idempotent; a different key for the same node is refused.
            assert!(!trust.enroll_peer_key(node, key).await.unwrap());
            assert_eq!(
                trust
                    .enroll_peer_key(node, other_key)
                    .await
                    .unwrap_err()
                    .kind(),
                ErrorKind::AuthenticationFailed
            );
            check(key).unwrap();
            // The all-zero identity is refused before any state change.
            assert!(
                trust
                    .enroll_peer_key(NodeId::from_bytes([0; 16]), key)
                    .await
                    .is_err()
            );
            // Forget removes both the trust and the pin.
            trust.forget_peer(node).await.unwrap();
            assert!(check(key).is_err());
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn legacy_trust_only_runtime_cannot_accept_cryptographic_membership() {
        let (root, store) = temp_store("p7-no-store");
        let network = NetworkId::from_bytes([0x51; 16]);
        let node = NodeId::from_bytes([0x61; 16]);
        let digest = SpkiSha256::from_verified_certificate_digest([0x71; 32]).unwrap();
        seed_current_network(&store, network);
        let trust = DurablePeerTrustOperations::new(store);
        futures_lite::future::block_on(trust.trust_peer(node)).unwrap();
        assert_eq!(
            trust
                .require_approved_pinned_tls_peer(network, node, digest)
                .unwrap_err()
                .kind(),
            ErrorKind::AuthenticationFailed,
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enrollment_requires_durable_approval_and_independent_matching_key() {
        futures_lite::future::block_on(async {
            let (root, store) = temp_store("p7-manual-confirmation");
            let network = NetworkId::from_bytes([0x31; 16]);
            let node = NodeId::from_bytes([0x32; 16]);
            let other = NodeId::from_bytes([0x33; 16]);
            let key = SpkiSha256::from_verified_certificate_digest([0x41; 32]).unwrap();
            let mismatch = SpkiSha256::from_verified_certificate_digest([0x42; 32]).unwrap();
            seed_current_network(&store, network);
            let trust = DurablePeerTrustOperations::with_peer_pin_store(
                store,
                PeerPinStore::new(root.join("enrollment-pins")),
            );
            let enroll = |node, verified, confirmed| {
                trust.enroll_verified_key(network, node, verified, confirmed)
            };
            assert_eq!(
                enroll(node, key, key).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed
            );
            trust.trust_peer(node).await.unwrap();
            assert_eq!(
                enroll(node, key, mismatch).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed
            );
            assert!(trust.pin_store.as_ref().unwrap().load().unwrap().is_empty());
            assert_eq!(enroll(node, key, key), Ok(true));
            assert_eq!(enroll(node, key, key), Ok(false));
            assert_eq!(
                enroll(other, key, key).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed
            );
            trust.trust_peer(other).await.unwrap();
            assert_eq!(
                enroll(other, key, key).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed
            );
            assert_eq!(
                enroll(node, mismatch, mismatch).unwrap_err().kind(),
                ErrorKind::AuthenticationFailed
            );
            assert_eq!(trust.pin_store.as_ref().unwrap().load().unwrap().len(), 1);
            trust.forget_peer(node).await.unwrap();
            assert!(trust.pin_store.as_ref().unwrap().load().unwrap().is_empty());
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn key_pin_approval_is_scoped_to_active_network() {
        let (root, store) = temp_store("p7-network-scope");
        let network = NetworkId::from_bytes([0x51; 16]);
        let other = NetworkId::from_bytes([0x61; 16]);
        let node = NodeId::from_bytes([0x72; 16]);
        let fingerprint = SpkiSha256::from_verified_certificate_digest([0x73; 32]).unwrap();
        seed_current_network(&store, network);
        let pin_store = PeerPinStore::new(root.join("peer-keys"));
        let trust = DurablePeerTrustOperations::with_peer_pin_store(store, pin_store);
        let mut pins = PeerKeyPins::new();
        pins.enroll_verified(other, node, fingerprint).unwrap();
        trust.pin_store.as_ref().unwrap().save(&pins).unwrap();
        assert_eq!(
            trust
                .require_approved_pinned_tls_peer(other, node, fingerprint)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_enrollments_do_not_lose_other_peer_pins_or_trust() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let (root, store) = temp_store("concurrent-enroll");
        let network = NetworkId::from_bytes([0x18; 16]);
        seed_current_network(&store, network);
        let trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            store,
            PeerPinStore::new(root.join("peer-keys")),
        ));
        let count = 12;
        let barrier = Arc::new(Barrier::new(count));
        let mut threads = Vec::new();
        for index in 1..=count {
            let operations = Arc::clone(&trust);
            let gate = Arc::clone(&barrier);
            threads.push(thread::spawn(move || {
                let node = NodeId::from_bytes([index as u8; 16]);
                let pin = SpkiSha256::from_verified_certificate_digest([index as u8; 32]).unwrap();
                gate.wait();
                futures_lite::future::block_on(operations.trust_peer(node)).unwrap();
                assert_eq!(
                    operations.enroll_verified_key(network, node, pin, pin),
                    Ok(true)
                );
            }));
        }
        for task in threads {
            task.join().unwrap();
        }
        let pins = trust.pin_store.as_ref().unwrap().load().unwrap();
        assert_eq!(pins.len(), count);
        for index in 1..=count {
            let node = NodeId::from_bytes([index as u8; 16]);
            let pin = SpkiSha256::from_verified_certificate_digest([index as u8; 32]).unwrap();
            trust
                .require_approved_pinned_tls_peer(network, node, pin)
                .unwrap();
        }

        // A completed revocation must erase the pin; reapproving a logical
        // node cannot revive it. The operation lock also protects this path.
        let revoked = NodeId::from_bytes([1; 16]);
        futures_lite::future::block_on(trust.forget_peer(revoked)).unwrap();
        futures_lite::future::block_on(trust.trust_peer(revoked)).unwrap();
        let pin = SpkiSha256::from_verified_certificate_digest([1; 32]).unwrap();
        assert_eq!(
            trust
                .require_approved_pinned_tls_peer(network, revoked, pin)
                .unwrap_err()
                .kind(),
            ErrorKind::AuthenticationFailed
        );
        assert_eq!(
            trust.pin_store.as_ref().unwrap().load().unwrap().len(),
            count - 1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inbound_pin_snapshot_ignores_unapproved_and_foreign_network_enrollments() {
        let (root, store) = temp_store("inbound-allowlist");
        let network = NetworkId::from_bytes([0x81; 16]);
        let foreign_network = NetworkId::from_bytes([0x82; 16]);
        let approved = NodeId::from_bytes([0x83; 16]);
        let unapproved = NodeId::from_bytes([0x84; 16]);
        let foreign_node = NodeId::from_bytes([0x85; 16]);
        let approved_key = SpkiSha256::from_verified_certificate_digest([0x86; 32]).unwrap();
        let stale_key = SpkiSha256::from_verified_certificate_digest([0x87; 32]).unwrap();
        let foreign_key = SpkiSha256::from_verified_certificate_digest([0x88; 32]).unwrap();
        seed_current_network(&store, network);
        let trust = DurablePeerTrustOperations::with_peer_pin_store(
            store,
            PeerPinStore::new(root.join("peer-keys")),
        );
        futures_lite::future::block_on(trust.trust_peer(approved)).unwrap();

        // Simulate a historical/orphaned enrollment record: possession of a
        // persisted pin must not imply current approval or network membership.
        let mut pins = PeerKeyPins::new();
        pins.enroll_verified(network, approved, approved_key)
            .unwrap();
        pins.enroll_verified(network, unapproved, stale_key)
            .unwrap();
        pins.enroll_verified(foreign_network, foreign_node, foreign_key)
            .unwrap();
        trust.pin_store.as_ref().unwrap().save(&pins).unwrap();
        assert_eq!(
            trust.approved_control_pins(network).unwrap(),
            vec![PinnedControlTlsPeer::new(network, approved, approved_key).unwrap()]
        );
        assert_eq!(
            trust
                .approved_control_pins(foreign_network)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );

        // Revocation must remove the sole eligible pin; a subsequent
        // NodeId-only approval cannot silently re-enroll that key.
        futures_lite::future::block_on(trust.forget_peer(approved)).unwrap();
        futures_lite::future::block_on(trust.trust_peer(approved)).unwrap();
        assert!(trust.approved_control_pins(network).unwrap().is_empty());

        // Corrupt durable key material is an error, never an empty allowlist
        // that a caller could confuse with a successful authorization read.
        fs::write(
            root.join("peer-keys/peer-key-pins-v1"),
            b"invalid pin state",
        )
        .unwrap();
        assert!(trust.approved_control_pins(network).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn approval_for_wrong_network_is_rejected() {
        let (root, store) = temp_store("wrong-network");
        let network = NetworkId::from_bytes([0x71; 16]);
        seed_current_network(&store, network);
        let operations = DurablePeerTrustOperations::new(store);
        let error = operations
            .require_peer_approved(
                NetworkId::from_bytes([0x72; 16]),
                NodeId::from_bytes([0x73; 16]),
            )
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn durable_trust_mutations_fail_closed_while_another_process_holds_lock() {
        let (root, store) = temp_store("process-lock");
        let network = NetworkId::from_bytes([0x21; 16]);
        let node = NodeId::from_bytes([0x22; 16]);
        seed_current_network(&store, network);
        let pin_path = root.join("peer-pins");
        let external = PeerPinStore::new(&pin_path);
        let guard = external.try_lock().unwrap();
        let trust =
            DurablePeerTrustOperations::with_peer_pin_store(store, PeerPinStore::new(pin_path));
        let error = futures_lite::future::block_on(trust.trust_peer(node)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(
            trust
                .require_approved_pinned_tls_peer(
                    network,
                    node,
                    SpkiSha256::from_verified_certificate_digest([0x31; 32]).unwrap()
                )
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        drop(guard);
        futures_lite::future::block_on(trust.trust_peer(node)).unwrap();
        trust.require_peer_approved(network, node).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn confirmed_enrollment_expiring_while_waiting_for_state_lock_writes_no_pin() {
        use std::sync::{Arc, mpsc};
        use std::thread;
        use std::time::Duration;

        let (root, store) = temp_store("expired-lock-wait");
        let network = NetworkId::from_bytes([0x91; 16]);
        let node = NodeId::from_bytes([0x92; 16]);
        let key = SpkiSha256::from_verified_certificate_digest([0x93; 32]).unwrap();
        seed_current_network(&store, network);
        let trust = Arc::new(DurablePeerTrustOperations::with_peer_pin_store(
            store,
            PeerPinStore::new(root.join("peer-keys")),
        ));
        futures_lite::future::block_on(trust.trust_peer(node)).unwrap();

        let guard = trust.state_lock.lock().unwrap();
        let expiry = Instant::now() + Duration::from_millis(25);
        let copy = Arc::clone(&trust);
        let (started_tx, started_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            copy.enroll_key_inner(network, node, key, key, Some(expiry))
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(70));
        drop(guard);

        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            ErrorKind::AuthenticationFailed
        );
        assert!(trust.pin_store.as_ref().unwrap().load().unwrap().is_empty());

        // Only a fresh deadline may authorize the same independently
        // approved and matching key after the contention clears.
        assert_eq!(
            trust.enroll_key_inner(
                network,
                node,
                key,
                key,
                Some(Instant::now() + Duration::from_secs(5))
            ),
            Ok(true)
        );
        assert_eq!(trust.pin_store.as_ref().unwrap().load().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    mod joiner {
        use super::*;
        use crate::{PendingJoinHost, PendingJoinStore};

        fn pin(value: u8) -> SpkiSha256 {
            SpkiSha256::from_verified_certificate_digest([value; 32]).unwrap()
        }

        struct Fixture {
            root: std::path::PathBuf,
            pending: Arc<PendingJoinOperations>,
            trust: DurablePeerTrustOperations,
            network: NetworkId,
            host: NodeId,
        }

        fn fixture(label: &str, with_pending: bool) -> Fixture {
            let (root, store) = temp_store(label);
            let pending = Arc::new(PendingJoinOperations::new(
                NetworkMembershipStore::new(root.join("memberships-v1")),
                PendingJoinStore::new(root.join("pending")),
            ));
            let mut trust = DurablePeerTrustOperations::with_peer_pin_store(
                store,
                PeerPinStore::new(root.join("pins")),
            );
            if with_pending {
                trust = trust.with_pending_join(pending.clone());
            }
            Fixture {
                root,
                pending,
                trust,
                network: NetworkId::from_bytes([0x51; 16]),
                host: NodeId::from_bytes([0x62; 16]),
            }
        }

        #[test]
        fn recorded_host_and_exact_key_authorize_a_node_in_no_network() {
            let f = fixture("joiner-ok", true);
            f.pending
                .record_host(PendingJoinHost::new(f.network, f.host, pin(7)).unwrap())
                .unwrap();
            assert_eq!(
                f.trust
                    .with_approved_pinned_tls_peer(f.network, f.host, pin(7), || 42)
                    .unwrap(),
                42
            );
            for (network, node, key) in [
                (NetworkId::from_bytes([0x52; 16]), f.host, pin(7)),
                (f.network, NodeId::from_bytes([0x63; 16]), pin(7)),
                (f.network, f.host, pin(8)),
            ] {
                assert_eq!(
                    f.trust
                        .require_approved_pinned_tls_peer(network, node, key)
                        .unwrap_err()
                        .kind(),
                    ErrorKind::AuthenticationFailed
                );
            }
            let _ = fs::remove_dir_all(f.root);
        }

        #[test]
        fn without_a_record_or_without_the_fallback_a_non_member_is_refused() {
            let none = fixture("joiner-none", true);
            assert!(
                none.trust
                    .require_approved_pinned_tls_peer(none.network, none.host, pin(7))
                    .is_err()
            );
            let disabled = fixture("joiner-off", false);
            disabled
                .pending
                .record_host(PendingJoinHost::new(disabled.network, disabled.host, pin(7)).unwrap())
                .unwrap();
            // The trust object was built without the fallback, so the record is ignored.
            assert!(
                disabled
                    .trust
                    .require_approved_pinned_tls_peer(disabled.network, disabled.host, pin(7))
                    .is_err()
            );
            let _ = fs::remove_dir_all(none.root);
            let _ = fs::remove_dir_all(disabled.root);
        }

        #[test]
        fn once_a_member_only_member_rules_apply_and_the_record_is_ignored() {
            let f = fixture("joiner-member", true);
            f.pending
                .record_host(PendingJoinHost::new(f.network, f.host, pin(7)).unwrap())
                .unwrap();
            seed_current_network(
                &NetworkMembershipStore::new(f.root.join("memberships-v1")),
                f.network,
            );
            // Now a member: the host is neither approved nor pinned in the member stores.
            assert!(
                f.trust
                    .require_approved_pinned_tls_peer(f.network, f.host, pin(7))
                    .is_err()
            );
            let _ = fs::remove_dir_all(f.root);
        }
    }
}

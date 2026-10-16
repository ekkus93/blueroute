use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard};

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId};
use blueroute_linux::{BluetoothBackend, BluezBackend, NetworkMembershipStore, PeerHandle};
use blueroute_protocol::peer_pin::SpkiSha256;

use crate::peer_pin_store::PeerPinFileLock;
use crate::{PeerPinStore, PinnedControlTlsPeer, current_network};

pub type PeerTrustFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

pub trait PeerTrustOperations: Send + Sync {
    fn trust_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()>;
    fn forget_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()>;
}

pub struct DurablePeerTrustOperations {
    store: NetworkMembershipStore,
    pin_store: Option<PeerPinStore>,
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
            state_lock: Mutex::new(()),
        }
    }

    /// Enable durable key enrollment lookups. Legacy trust-only construction
    /// remains valid for P6-003, but cannot grant cryptographic P7 access.
    pub fn with_peer_pin_store(store: NetworkMembershipStore, pins: PeerPinStore) -> Self {
        Self {
            store,
            pin_store: Some(pins),
            state_lock: Mutex::new(()),
        }
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
        )
    }

    /// Synthetic policy seam for tests and internal callers. Production
    /// enrollment must use the opaque TLS-verified peer entry point above.
    #[cfg(test)]
    pub(crate) fn enroll_verified_key(
        &self,
        network: NetworkId,
        node: NodeId,
        verified_tls_spki: SpkiSha256,
        confirmed_spki: SpkiSha256,
    ) -> Result<bool, CoreError> {
        self.enroll_key_inner(network, node, verified_tls_spki, confirmed_spki)
    }

    fn enroll_key_inner(
        &self,
        network: NetworkId,
        node: NodeId,
        verified_tls_spki: SpkiSha256,
        confirmed_spki: SpkiSha256,
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
        let changed = pins
            .enroll_verified(network, node, verified_tls_spki)
            .map_err(|_| {
                CoreError::new(
                    ErrorKind::AuthenticationFailed,
                    "peer identity or key conflicts with an existing enrollment",
                )
            })?;
        if changed {
            store.save(&pins)?;
        }
        Ok(changed)
    }
}

impl PeerTrustOperations for DurablePeerTrustOperations {
    fn trust_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()> {
        Box::pin(async move {
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
}

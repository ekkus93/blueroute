use std::future::Future;
use std::pin::Pin;

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId};
use blueroute_linux::{BluetoothBackend, BluezBackend, NetworkMembershipStore, PeerHandle};
use blueroute_protocol::peer_pin::{PeerKeyPins, SpkiSha256};

use crate::current_network;

pub type PeerTrustFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

pub trait PeerTrustOperations: Send + Sync {
    fn trust_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()>;
    fn forget_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()>;
}

pub struct DurablePeerTrustOperations {
    store: NetworkMembershipStore,
}

impl DurablePeerTrustOperations {
    pub fn new(store: NetworkMembershipStore) -> Self {
        Self { store }
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

    /// P7 key-binding gate over the existing durable P6-003 approval policy.
    ///
    /// The TLS layer must first validate the presented certificate and proof
    /// of private-key possession, then provide its verified SPKI-SHA256. A
    /// NodeId-only trust entry is insufficient: a separate explicit key pin
    /// must also exist for precisely this (NetworkId, NodeId).
    ///
    /// This is a policy integration only: there is no TLS handshake, durable
    /// key-enrollment store, or production JoinNetwork activation yet.
    pub fn require_approved_pinned_tls_peer(
        &self,
        network: NetworkId,
        node: NodeId,
        verified_spki_sha256: SpkiSha256,
        pins: &PeerKeyPins,
    ) -> Result<(), CoreError> {
        self.require_peer_approved(network, node)?;
        pins.authorize(network, node, true, verified_spki_sha256)
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::AuthenticationFailed,
                    "peer has no matching explicitly enrolled BlueRoute cryptographic identity",
                    error.to_string(),
                )
            })
    }
}

impl PeerTrustOperations for DurablePeerTrustOperations {
    fn trust_peer(&self, node: NodeId) -> PeerTrustFuture<'_, ()> {
        Box::pin(async move {
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
            let mut registry = self.store.load()?;
            let network = current_network(&registry)?.ok_or_else(no_current_network)?;
            let membership = registry.network_mut(&network).ok_or_else(|| {
                CoreError::new(
                    ErrorKind::InvalidState,
                    "current BlueRoute network is missing from durable membership state",
                )
            })?;
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
            let trust = DurablePeerTrustOperations::new(store);
            trust.trust_peer(node).await.unwrap();

            let mut pins = PeerKeyPins::new();
            assert_eq!(
                trust
                    .require_approved_pinned_tls_peer(network, node, fingerprint, &pins)
                    .unwrap_err()
                    .kind(),
                ErrorKind::AuthenticationFailed,
            );
            pins.enroll_verified(network, node, fingerprint).unwrap();
            assert_eq!(
                trust
                    .require_approved_pinned_tls_peer(network, node, wrong_fingerprint, &pins,)
                    .unwrap_err()
                    .kind(),
                ErrorKind::AuthenticationFailed,
            );
            trust
                .require_approved_pinned_tls_peer(network, node, fingerprint, &pins)
                .unwrap();
            trust.forget_peer(node).await.unwrap();
            assert_eq!(
                trust
                    .require_approved_pinned_tls_peer(network, node, fingerprint, &pins)
                    .unwrap_err()
                    .kind(),
                ErrorKind::AuthenticationFailed,
            );
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
        let trust = DurablePeerTrustOperations::new(store);
        let mut pins = PeerKeyPins::new();
        pins.enroll_verified(other, node, fingerprint).unwrap();
        assert_eq!(
            trust
                .require_approved_pinned_tls_peer(other, node, fingerprint, &pins)
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
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
}

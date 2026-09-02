use std::future::Future;
use std::pin::Pin;

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId};
use blueroute_linux::{BluetoothBackend, BluezBackend, NetworkMembershipStore, PeerHandle};

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

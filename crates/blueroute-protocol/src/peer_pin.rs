//! Per-network public-key pinning policy for the future authenticated P7 control channel.
//!
//! No cryptography or TLS verification is performed here. The caller must
//! compute the SHA-256 digest of the peer certificate's SubjectPublicKeyInfo
//! **after** the TLS stack proves possession of the corresponding private key.
//! Never pass a claimed fingerprint from an untrusted hello, LE advertisement,
//! IP address, Bluetooth name, or NodeId into `authorize`.
//!
//! This in-memory policy is not a durable enrollment database. Persistent,
//! user-approved key enrollment and revocation still need implementation before
//! production JoinNetwork is enabled.

use std::collections::BTreeMap;
use std::fmt;

use blueroute_core::{NetworkId, NodeId};

/// Digest of DER-encoded SubjectPublicKeyInfo (SPKI), produced by a trusted
/// certificate verifier, not parsed from untrusted control messages.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SpkiSha256([u8; 32]);

impl SpkiSha256 {
    pub fn from_verified_certificate_digest(digest: [u8; 32]) -> Result<Self, PeerPinError> {
        if digest.iter().all(|byte| *byte == 0) {
            return Err(PeerPinError::InvalidFingerprint);
        }
        Ok(Self(digest))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerPinError {
    InvalidFingerprint,
    UnknownEnrollment,
    KeyConflict,
    NotApprovedForNetwork,
    FingerprintMismatch,
}

impl fmt::Display for PeerPinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFingerprint => f.write_str("invalid peer key fingerprint"),
            Self::UnknownEnrollment => f.write_str("no verified peer key enrollment exists"),
            Self::KeyConflict => f.write_str("peer key changed; explicit re-enrollment required"),
            Self::NotApprovedForNetwork => f.write_str("peer lacks network membership approval"),
            Self::FingerprintMismatch => {
                f.write_str("authenticated peer public key differs from pin")
            }
        }
    }
}

impl std::error::Error for PeerPinError {}

/// Security-sensitive mapping of (NetworkId, NodeId) to explicit enrolled key.
/// An existing P6-003 NodeId-only trust record does **not** populate this map.
#[derive(Clone, Debug, Default)]
pub struct PeerKeyPins {
    pins: BTreeMap<(NetworkId, NodeId), SpkiSha256>,
}

impl PeerKeyPins {
    pub fn new() -> Self {
        Self::default()
    }

    /// Caller MUST obtain enrollment authorization from an independent
    /// user-confirmed key-verification ceremony; this method cannot provide it.
    /// Prevents silent overwrite/key substitution across restarts and sessions.
    pub fn enroll_verified(
        &mut self,
        network: NetworkId,
        node: NodeId,
        pin: SpkiSha256,
    ) -> Result<bool, PeerPinError> {
        match self.pins.get(&(network, node)) {
            Some(existing) if *existing == pin => Ok(false),
            Some(_) => Err(PeerPinError::KeyConflict),
            None => {
                self.pins.insert((network, node), pin);
                Ok(true)
            }
        }
    }

    /// Forgetting trust must also drop its key pin; changes require a new
    /// explicitly approved enrollment ceremony, not an implicit rotation.
    pub fn revoke(&mut self, network: NetworkId, node: NodeId) -> bool {
        self.pins.remove(&(network, node)).is_some()
    }

    /// A positive check requires BOTH explicit current network approval from
    /// the durable membership store AND verified TLS certificate key equality.
    /// The TLS layer, not this map, must verify proof of key possession.
    pub fn authorize(
        &self,
        network: NetworkId,
        node: NodeId,
        current_network_approval: bool,
        verified_peer_spki_sha256: SpkiSha256,
    ) -> Result<(), PeerPinError> {
        if !current_network_approval {
            return Err(PeerPinError::NotApprovedForNetwork);
        }
        let expected = self
            .pins
            .get(&(network, node))
            .ok_or(PeerPinError::UnknownEnrollment)?;
        if *expected != verified_peer_spki_sha256 {
            return Err(PeerPinError::FingerprintMismatch);
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.pins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn fingerprint(value: u8) -> SpkiSha256 {
        SpkiSha256::from_verified_certificate_digest([value; 32]).unwrap()
    }

    #[test]
    fn unapproved_node_id_only_trust_fails_closed() {
        let pins = PeerKeyPins::new();
        assert_eq!(
            pins.authorize(network(1), node(2), true, fingerprint(3)),
            Err(PeerPinError::UnknownEnrollment)
        );
        assert!(SpkiSha256::from_verified_certificate_digest([0; 32]).is_err());
    }

    #[test]
    fn exact_pinned_approved_peer_is_accepted_only_for_its_network_and_node() {
        let mut pins = PeerKeyPins::new();
        assert_eq!(
            pins.enroll_verified(network(1), node(2), fingerprint(3)),
            Ok(true)
        );
        assert_eq!(
            pins.enroll_verified(network(1), node(2), fingerprint(3)),
            Ok(false)
        );
        assert_eq!(
            pins.authorize(network(1), node(2), true, fingerprint(3)),
            Ok(())
        );
        assert_eq!(
            pins.authorize(network(1), node(2), false, fingerprint(3)),
            Err(PeerPinError::NotApprovedForNetwork)
        );
        for (network, node) in [(network(4), node(2)), (network(1), node(5))] {
            assert_eq!(
                pins.authorize(network, node, true, fingerprint(3)),
                Err(PeerPinError::UnknownEnrollment)
            );
        }
    }

    #[test]
    fn wrong_tls_key_and_silent_key_rotation_are_rejected() {
        let mut pins = PeerKeyPins::new();
        pins.enroll_verified(network(1), node(2), fingerprint(3))
            .unwrap();
        assert_eq!(
            pins.enroll_verified(network(1), node(2), fingerprint(4)),
            Err(PeerPinError::KeyConflict)
        );
        assert_eq!(
            pins.authorize(network(1), node(2), true, fingerprint(4)),
            Err(PeerPinError::FingerprintMismatch)
        );
        assert_eq!(
            pins.authorize(network(1), node(2), true, fingerprint(3)),
            Ok(())
        );
    }

    #[test]
    fn revocation_is_network_scoped_and_idempotent() {
        let mut pins = PeerKeyPins::new();
        pins.enroll_verified(network(1), node(2), fingerprint(3))
            .unwrap();
        pins.enroll_verified(network(4), node(2), fingerprint(3))
            .unwrap();
        assert!(pins.revoke(network(1), node(2)));
        assert!(!pins.revoke(network(1), node(2)));
        assert_eq!(
            pins.authorize(network(1), node(2), true, fingerprint(3)),
            Err(PeerPinError::UnknownEnrollment)
        );
        assert_eq!(
            pins.authorize(network(4), node(2), true, fingerprint(3)),
            Ok(())
        );
        assert_eq!(pins.len(), 1);
    }
}

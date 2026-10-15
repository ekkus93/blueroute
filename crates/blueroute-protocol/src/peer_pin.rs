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

    /// Strict canonical fingerprint syntax for a future *independently*
    /// authenticated human comparison. Parsing is NOT verification or
    /// enrollment and must never replace TLS proof of key possession.
    pub fn parse_canonical_hex(text: &str) -> Result<Self, PeerPinError> {
        if text.len() != 64 {
            return Err(PeerPinError::InvalidFingerprint);
        }
        let mut digest = [0_u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            let bytes = text.as_bytes();
            let hex = |value| match value {
                b'0'..=b'9' => Some(value - b'0'),
                b'a'..=b'f' => Some(value - b'a' + 10),
                _ => None,
            };
            let high = hex(bytes[index * 2]).ok_or(PeerPinError::InvalidFingerprint)?;
            let low = hex(bytes[index * 2 + 1]).ok_or(PeerPinError::InvalidFingerprint)?;
            *byte = (high << 4) | low;
        }
        Self::from_verified_certificate_digest(digest)
    }

    pub fn to_canonical_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(64);
        for byte in self.0 {
            text.push(char::from(HEX[usize::from(byte >> 4)]));
            text.push(char::from(HEX[usize::from(byte & 15)]));
        }
        text
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerPinError {
    InvalidFingerprint,
    InvalidIdentity,
    UnknownEnrollment,
    KeyConflict,
    NotApprovedForNetwork,
    FingerprintMismatch,
}

impl fmt::Display for PeerPinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFingerprint => f.write_str("invalid peer key fingerprint"),
            Self::InvalidIdentity => {
                f.write_str("zero network or node identity cannot be enrolled")
            }
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
        if network == NetworkId::from_bytes([0; 16]) || node == NodeId::from_bytes([0; 16]) {
            return Err(PeerPinError::InvalidIdentity);
        }
        // One enrolled TLS key must not impersonate multiple logical NodeIds
        // in the same network. This holds even if both NodeIds have legacy
        // trust records. Cross-network key reuse is a separate policy choice.
        if self
            .pins
            .iter()
            .any(|(&(enrolled_network, enrolled_node), existing)| {
                enrolled_network == network && enrolled_node != node && *existing == pin
            })
        {
            return Err(PeerPinError::KeyConflict);
        }
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
        if network == NetworkId::from_bytes([0; 16]) || node == NodeId::from_bytes([0; 16]) {
            return Err(PeerPinError::InvalidIdentity);
        }
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

    /// Stable iteration for versioned owner-only persisted key pin records.
    /// Loading an entry is NOT the same as approving a peer for membership.
    pub fn entries(&self) -> impl Iterator<Item = (NetworkId, NodeId, SpkiSha256)> + '_ {
        self.pins
            .iter()
            .map(|(&(network, node), &pin)| (network, node, pin))
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
    fn canonical_public_key_fingerprint_round_trip_and_invalid_input() {
        let key = fingerprint(0xab);
        let canonical = "ab".repeat(32);
        assert_eq!(key.to_canonical_hex(), canonical);
        assert_eq!(SpkiSha256::parse_canonical_hex(&canonical), Ok(key));
        for wrong in [
            "AB".repeat(32),
            "ab".repeat(31),
            "ab".repeat(33),
            format!("{}!", "ab".repeat(31)),
            "00".repeat(32),
            " ab".repeat(21),
        ] {
            assert_eq!(
                SpkiSha256::parse_canonical_hex(&wrong),
                Err(PeerPinError::InvalidFingerprint)
            );
        }
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
    fn zero_network_and_node_id_never_enter_or_authorize_enrollment_registry() {
        let mut pins = PeerKeyPins::new();
        let valid = fingerprint(3);
        for (network, node) in [(network(0), node(2)), (network(1), node(0))] {
            assert_eq!(
                pins.enroll_verified(network, node, valid),
                Err(PeerPinError::InvalidIdentity)
            );
            assert_eq!(
                pins.authorize(network, node, true, valid),
                Err(PeerPinError::InvalidIdentity)
            );
        }
        assert!(pins.is_empty());
        assert_eq!(pins.enroll_verified(network(1), node(2), valid), Ok(true));
        assert_eq!(pins.len(), 1);
        assert_eq!(pins.authorize(network(1), node(2), true, valid), Ok(()));
    }

    #[test]
    fn same_tls_key_cannot_authorize_two_node_ids_in_one_network() {
        let mut pins = PeerKeyPins::new();
        assert_eq!(
            pins.enroll_verified(network(1), node(2), fingerprint(3)),
            Ok(true)
        );
        assert_eq!(
            pins.enroll_verified(network(1), node(4), fingerprint(3)),
            Err(PeerPinError::KeyConflict)
        );
        assert_eq!(pins.len(), 1);
        assert_eq!(
            pins.authorize(network(1), node(4), true, fingerprint(3)),
            Err(PeerPinError::UnknownEnrollment)
        );
        // The same physical node may be independently enrolled in a
        // different BlueRoute network without conferring approval to peers.
        assert_eq!(
            pins.enroll_verified(network(9), node(2), fingerprint(3)),
            Ok(true)
        );
        assert_eq!(
            pins.enroll_verified(network(1), node(4), fingerprint(5)),
            Ok(true)
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

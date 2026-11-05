//! Node identity keys, key-derived `NodeId`, Noise static-key binding, and membership
//! credentials (ADR 0001). Pure functions over bytes: no I/O and no randomness. Callers
//! supply the 32-byte private seed from the platform random source.

use std::fmt;

use blueroute_core::{NetworkId, NodeId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PUBLIC_KEY_BYTES: usize = 32;
pub const SIGNATURE_BYTES: usize = 64;
pub const SEED_BYTES: usize = 32;

const NODE_ID_DOMAIN: &[u8] = b"blueroute/node-id/v1";
const NOISE_BINDING_DOMAIN: &[u8] = b"blueroute/noise-binding/v1";
const CREDENTIAL_DOMAIN: &[u8] = b"blueroute/credential/v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoError {
    InvalidPublicKey,
    InvalidSignature,
    CredentialNotYetValid,
    CredentialExpired,
    CredentialNotForNode,
    CredentialWrongNetwork,
    InvalidValidityWindow,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPublicKey => "invalid Ed25519 public key",
            Self::InvalidSignature => "signature verification failed",
            Self::CredentialNotYetValid => "membership credential is not yet valid",
            Self::CredentialExpired => "membership credential has expired",
            Self::CredentialNotForNode => "membership credential names a different node",
            Self::CredentialWrongNetwork => "membership credential is for a different network",
            Self::InvalidValidityWindow => "membership credential has an invalid validity window",
        })
    }
}

impl std::error::Error for CryptoError {}

/// `NodeId = first 16 bytes of SHA-256("blueroute/node-id/v1" || public_key)`.
pub fn node_id_from_public_key(public: &[u8; PUBLIC_KEY_BYTES]) -> NodeId {
    let digest = Sha256::new()
        .chain_update(NODE_ID_DOMAIN)
        .chain_update(public)
        .finalize();
    let mut id = [0_u8; 16];
    id.copy_from_slice(&digest[..16]);
    NodeId::from_bytes(id)
}

/// A node's long-term Ed25519 identity. `Debug` never prints key material.
pub struct NodeIdentity {
    signing: SigningKey,
}

impl fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeIdentity({}, <redacted>)", self.node_id())
    }
}

impl NodeIdentity {
    pub fn from_seed(seed: &[u8; SEED_BYTES]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Private seed for persistence in the secret store. Handle as a secret.
    pub fn seed(&self) -> [u8; SEED_BYTES] {
        self.signing.to_bytes()
    }

    pub fn public_key(&self) -> [u8; PUBLIC_KEY_BYTES] {
        self.signing.verifying_key().to_bytes()
    }

    pub fn node_id(&self) -> NodeId {
        node_id_from_public_key(&self.public_key())
    }

    fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_BYTES] {
        self.signing.sign(message).to_bytes()
    }

    /// Bind the Noise static key used in a handshake to this identity.
    pub fn sign_noise_binding(&self, noise_static: &[u8; 32]) -> [u8; SIGNATURE_BYTES] {
        self.sign(&noise_binding_message(noise_static))
    }

    /// Issue a membership credential. Only the network authority should call this.
    pub fn issue_credential(
        &self,
        network: NetworkId,
        node: NodeId,
        not_before: u64,
        not_after: u64,
    ) -> Result<Credential, CryptoError> {
        if not_after <= not_before {
            return Err(CryptoError::InvalidValidityWindow);
        }
        let signature = self.sign(&credential_message(network, node, not_before, not_after));
        Ok(Credential {
            network,
            node,
            not_before,
            not_after,
            signature: hex(&signature),
        })
    }
}

fn noise_binding_message(noise_static: &[u8; 32]) -> Vec<u8> {
    let mut message = NOISE_BINDING_DOMAIN.to_vec();
    message.extend_from_slice(noise_static);
    message
}

fn credential_message(
    network: NetworkId,
    node: NodeId,
    not_before: u64,
    not_after: u64,
) -> Vec<u8> {
    let mut message = CREDENTIAL_DOMAIN.to_vec();
    message.extend_from_slice(network.as_bytes());
    message.extend_from_slice(node.as_bytes());
    message.extend_from_slice(&not_before.to_be_bytes());
    message.extend_from_slice(&not_after.to_be_bytes());
    message
}

fn verifying_key(public: &[u8; PUBLIC_KEY_BYTES]) -> Result<VerifyingKey, CryptoError> {
    VerifyingKey::from_bytes(public).map_err(|_| CryptoError::InvalidPublicKey)
}

fn verify(
    public: &[u8; PUBLIC_KEY_BYTES],
    message: &[u8],
    signature: &[u8; SIGNATURE_BYTES],
) -> Result<(), CryptoError> {
    verifying_key(public)?
        .verify_strict(message, &Signature::from_bytes(signature))
        .map_err(|_| CryptoError::InvalidSignature)
}

/// Verify the Noise-binding signature and return the `NodeId` it proves.
pub fn verify_noise_binding(
    identity_public: &[u8; PUBLIC_KEY_BYTES],
    noise_static: &[u8; 32],
    signature: &[u8; SIGNATURE_BYTES],
) -> Result<NodeId, CryptoError> {
    verify(
        identity_public,
        &noise_binding_message(noise_static),
        signature,
    )?;
    Ok(node_id_from_public_key(identity_public))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex_signature(text: &str) -> Option<[u8; SIGNATURE_BYTES]> {
    if text.len() != SIGNATURE_BYTES * 2 || !text.is_ascii() {
        return None;
    }
    let mut out = [0_u8; SIGNATURE_BYTES];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// An authority-signed statement that `node` is a member of `network` for a time window
/// (seconds since the Unix epoch).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Credential {
    pub network: NetworkId,
    pub node: NodeId,
    pub not_before: u64,
    pub not_after: u64,
    /// Lowercase hex Ed25519 signature by the network authority.
    pub signature: String,
}

impl Credential {
    /// Verify against the pinned authority key. `skew_secs` tolerates unsynchronized clocks.
    pub fn verify(
        &self,
        authority: &[u8; PUBLIC_KEY_BYTES],
        expected_network: NetworkId,
        expected_node: NodeId,
        now: u64,
        skew_secs: u64,
    ) -> Result<(), CryptoError> {
        if self.network != expected_network {
            return Err(CryptoError::CredentialWrongNetwork);
        }
        if self.node != expected_node {
            return Err(CryptoError::CredentialNotForNode);
        }
        let signature = unhex_signature(&self.signature).ok_or(CryptoError::InvalidSignature)?;
        verify(
            authority,
            &credential_message(self.network, self.node, self.not_before, self.not_after),
            &signature,
        )?;
        if now.saturating_add(skew_secs) < self.not_before {
            return Err(CryptoError::CredentialNotYetValid);
        }
        if now > self.not_after.saturating_add(skew_secs) {
            return Err(CryptoError::CredentialExpired);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(b: u8) -> NodeIdentity {
        NodeIdentity::from_seed(&[b; SEED_BYTES])
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([7; 16])
    }

    #[test]
    fn node_id_is_deterministic_and_key_bound() {
        let a = identity(1);
        assert_eq!(a.node_id(), identity(1).node_id());
        assert_ne!(a.node_id(), identity(2).node_id());
        assert_eq!(a.node_id(), node_id_from_public_key(&a.public_key()));
    }

    #[test]
    fn debug_never_prints_the_private_seed() {
        let a = identity(0xAB);
        let text = format!("{a:?}");
        assert!(text.contains("redacted"));
        assert!(!text.contains(&hex(&a.seed())));
    }

    #[test]
    fn noise_binding_proves_the_node_id_and_rejects_other_keys() {
        let a = identity(1);
        let noise = [9_u8; 32];
        let sig = a.sign_noise_binding(&noise);
        assert_eq!(
            verify_noise_binding(&a.public_key(), &noise, &sig),
            Ok(a.node_id())
        );
        assert_eq!(
            verify_noise_binding(&a.public_key(), &[8_u8; 32], &sig),
            Err(CryptoError::InvalidSignature)
        );
        assert_eq!(
            verify_noise_binding(&identity(2).public_key(), &noise, &sig),
            Err(CryptoError::InvalidSignature)
        );
    }

    #[test]
    fn credential_verifies_within_its_window_and_with_bounded_skew() {
        let authority = identity(1);
        let member = identity(2);
        let c = authority
            .issue_credential(network(), member.node_id(), 1000, 2000)
            .unwrap();
        let check = |now, skew| {
            c.verify(
                &authority.public_key(),
                network(),
                member.node_id(),
                now,
                skew,
            )
        };
        assert_eq!(check(1500, 0), Ok(()));
        assert_eq!(check(999, 0), Err(CryptoError::CredentialNotYetValid));
        assert_eq!(check(999, 5), Ok(()));
        assert_eq!(check(2001, 0), Err(CryptoError::CredentialExpired));
        assert_eq!(check(2003, 5), Ok(()));
    }

    #[test]
    fn credential_is_bound_to_authority_network_node_and_content() {
        let authority = identity(1);
        let member = identity(2);
        let c = authority
            .issue_credential(network(), member.node_id(), 0, 10)
            .unwrap();
        let other = identity(3);
        assert_eq!(
            c.verify(&other.public_key(), network(), member.node_id(), 5, 0),
            Err(CryptoError::InvalidSignature)
        );
        assert_eq!(
            c.verify(
                &authority.public_key(),
                NetworkId::from_bytes([1; 16]),
                member.node_id(),
                5,
                0
            ),
            Err(CryptoError::CredentialWrongNetwork)
        );
        assert_eq!(
            c.verify(&authority.public_key(), network(), other.node_id(), 5, 0),
            Err(CryptoError::CredentialNotForNode)
        );
        let mut tampered = c.clone();
        tampered.not_after = 1_000_000;
        assert_eq!(
            tampered.verify(&authority.public_key(), network(), member.node_id(), 500, 0),
            Err(CryptoError::InvalidSignature)
        );
        let mut garbage = c;
        garbage.signature = "zz".into();
        assert_eq!(
            garbage.verify(&authority.public_key(), network(), member.node_id(), 5, 0),
            Err(CryptoError::InvalidSignature)
        );
    }

    #[test]
    fn credential_serde_round_trips_and_bad_windows_are_refused() {
        let authority = identity(1);
        let c = authority
            .issue_credential(network(), identity(2).node_id(), 1, 2)
            .unwrap();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Credential>(&json).unwrap(), c);
        assert_eq!(
            authority.issue_credential(network(), identity(2).node_id(), 5, 5),
            Err(CryptoError::InvalidValidityWindow)
        );
    }

    #[test]
    fn invalid_public_key_bytes_do_not_panic() {
        let mut bad = [0xFF_u8; PUBLIC_KEY_BYTES];
        bad[31] = 0xFF;
        let sig = [0_u8; SIGNATURE_BYTES];
        assert!(verify_noise_binding(&bad, &[0; 32], &sig).is_err());
    }
}

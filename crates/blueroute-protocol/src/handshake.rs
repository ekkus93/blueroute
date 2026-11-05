//! Authenticated control-session handshake (ADR 0001).
//!
//! `Noise_XX_25519_ChaChaPoly_SHA256` with a per-session Curve25519 static key. In messages 2
//! and 3 the sender's encrypted payload carries its Ed25519 identity key and a signature
//! binding the Noise static key to that identity, so the peer's `NodeId` is proven by key
//! possession and never taken from an address or a name. The state machine is sans-I/O: the
//! caller moves the handshake messages over the transport.

use std::fmt;

use blueroute_core::NodeId;
use snow::{Builder, HandshakeState, TransportState};

use crate::crypto::{
    CryptoError, NodeIdentity, PUBLIC_KEY_BYTES, SIGNATURE_BYTES, verify_noise_binding,
};

const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_SHA256";
const PAYLOAD_BYTES: usize = PUBLIC_KEY_BYTES + SIGNATURE_BYTES;
/// Noise limits a single message to 65535 bytes.
pub const MAX_NOISE_MESSAGE_BYTES: usize = 65_535;
const AEAD_TAG_BYTES: usize = 16;

#[derive(Debug)]
pub enum HandshakeError {
    /// Handshake step called out of order or after completion.
    OutOfOrder,
    Noise(String),
    /// The peer's identity payload was missing, short, or its signature did not verify.
    BadIdentity(CryptoError),
    MalformedIdentityPayload,
    /// The authenticated peer is not the node the caller expected to reach.
    UnexpectedPeer {
        expected: NodeId,
        actual: NodeId,
    },
    MessageTooLarge,
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfOrder => f.write_str("handshake step out of order"),
            Self::Noise(reason) => write!(f, "noise error: {reason}"),
            Self::BadIdentity(error) => write!(f, "peer identity rejected: {error}"),
            Self::MalformedIdentityPayload => f.write_str("malformed peer identity payload"),
            Self::UnexpectedPeer { expected, actual } => {
                write!(
                    f,
                    "authenticated peer {actual} is not the expected peer {expected}"
                )
            }
            Self::MessageTooLarge => f.write_str("message exceeds the Noise size limit"),
        }
    }
}

impl std::error::Error for HandshakeError {}

impl From<snow::Error> for HandshakeError {
    fn from(error: snow::Error) -> Self {
        Self::Noise(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Initiator,
    Responder,
}

pub struct Handshake {
    role: Role,
    state: HandshakeState,
    identity_payload: [u8; PAYLOAD_BYTES],
    expected_peer: Option<NodeId>,
    /// Messages processed so far (0..=3).
    step: u8,
    peer: Option<(NodeId, [u8; PUBLIC_KEY_BYTES])>,
}

impl fmt::Debug for Handshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handshake")
            .field("role", &self.role)
            .field("step", &self.step)
            .finish_non_exhaustive()
    }
}

impl Handshake {
    /// `expected_peer`, when known (for example from an authority-confirmed join), makes the
    /// handshake fail unless the authenticated peer is exactly that node.
    pub fn new(
        role: Role,
        identity: &NodeIdentity,
        expected_peer: Option<NodeId>,
    ) -> Result<Self, HandshakeError> {
        let params = NOISE_PARAMS
            .parse()
            .map_err(|e: snow::Error| HandshakeError::Noise(e.to_string()))?;
        let keypair = Builder::new(
            NOISE_PARAMS
                .parse()
                .map_err(|e: snow::Error| HandshakeError::Noise(e.to_string()))?,
        )
        .generate_keypair()?;
        let static_public: [u8; 32] = keypair
            .public
            .as_slice()
            .try_into()
            .map_err(|_| HandshakeError::Noise("unexpected static key length".into()))?;

        let mut identity_payload = [0_u8; PAYLOAD_BYTES];
        identity_payload[..PUBLIC_KEY_BYTES].copy_from_slice(&identity.public_key());
        identity_payload[PUBLIC_KEY_BYTES..]
            .copy_from_slice(&identity.sign_noise_binding(&static_public));

        let builder = Builder::new(params).local_private_key(&keypair.private)?;
        let state = match role {
            Role::Initiator => builder.build_initiator()?,
            Role::Responder => builder.build_responder()?,
        };
        Ok(Self {
            role,
            state,
            identity_payload,
            expected_peer,
            step: 0,
            peer: None,
        })
    }

    fn is_my_turn(&self) -> bool {
        match self.role {
            Role::Initiator => self.step.is_multiple_of(2),
            Role::Responder => self.step % 2 == 1,
        }
    }

    /// Produce the next outbound handshake message.
    pub fn write_message(&mut self) -> Result<Vec<u8>, HandshakeError> {
        if self.step >= 3 || !self.is_my_turn() {
            return Err(HandshakeError::OutOfOrder);
        }
        // Message 1 has no payload; messages 2 and 3 carry the identity proof.
        let payload: &[u8] = if self.step == 0 {
            &[]
        } else {
            &self.identity_payload
        };
        let mut buffer = vec![0_u8; MAX_NOISE_MESSAGE_BYTES];
        let len = self.state.write_message(payload, &mut buffer)?;
        buffer.truncate(len);
        self.step += 1;
        Ok(buffer)
    }

    /// Consume an inbound handshake message and verify the peer identity when it arrives.
    pub fn read_message(&mut self, message: &[u8]) -> Result<(), HandshakeError> {
        if self.step >= 3 || self.is_my_turn() {
            return Err(HandshakeError::OutOfOrder);
        }
        if message.len() > MAX_NOISE_MESSAGE_BYTES {
            return Err(HandshakeError::MessageTooLarge);
        }
        let mut payload = vec![0_u8; MAX_NOISE_MESSAGE_BYTES];
        let len = self.state.read_message(message, &mut payload)?;
        payload.truncate(len);
        self.step += 1;
        if self.step >= 2 {
            self.verify_peer(&payload)?;
        } else if !payload.is_empty() {
            // Message 1 is unauthenticated; it must carry no payload.
            return Err(HandshakeError::MalformedIdentityPayload);
        }
        Ok(())
    }

    fn verify_peer(&mut self, payload: &[u8]) -> Result<(), HandshakeError> {
        if payload.len() != PAYLOAD_BYTES {
            return Err(HandshakeError::MalformedIdentityPayload);
        }
        let mut public = [0_u8; PUBLIC_KEY_BYTES];
        public.copy_from_slice(&payload[..PUBLIC_KEY_BYTES]);
        let mut signature = [0_u8; SIGNATURE_BYTES];
        signature.copy_from_slice(&payload[PUBLIC_KEY_BYTES..]);
        let remote_static: [u8; 32] = self
            .state
            .get_remote_static()
            .and_then(|s| s.try_into().ok())
            .ok_or(HandshakeError::MalformedIdentityPayload)?;
        let node = verify_noise_binding(&public, &remote_static, &signature)
            .map_err(HandshakeError::BadIdentity)?;
        if let Some(expected) = self.expected_peer
            && expected != node
        {
            return Err(HandshakeError::UnexpectedPeer {
                expected,
                actual: node,
            });
        }
        self.peer = Some((node, public));
        Ok(())
    }

    pub fn is_finished(&self) -> bool {
        self.step == 3 && self.peer.is_some()
    }

    /// Convert a completed handshake into an encrypted session.
    pub fn finish(self) -> Result<SecureSession, HandshakeError> {
        let (peer, peer_public_key) = match (self.is_finished(), self.peer) {
            (true, Some(peer)) => peer,
            _ => return Err(HandshakeError::OutOfOrder),
        };
        Ok(SecureSession {
            transport: self.state.into_transport_mode()?,
            peer,
            peer_public_key,
        })
    }
}

/// AEAD-protected channel. Noise's per-direction nonce counters make captured ciphertext
/// unusable in another session and reject replays within this one.
pub struct SecureSession {
    transport: TransportState,
    peer: NodeId,
    peer_public_key: [u8; PUBLIC_KEY_BYTES],
}

impl fmt::Debug for SecureSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecureSession")
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

impl SecureSession {
    /// The proven identity of the remote node.
    pub fn peer(&self) -> NodeId {
        self.peer
    }

    pub fn peer_public_key(&self) -> [u8; PUBLIC_KEY_BYTES] {
        self.peer_public_key
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, HandshakeError> {
        if plaintext.len() + AEAD_TAG_BYTES > MAX_NOISE_MESSAGE_BYTES {
            return Err(HandshakeError::MessageTooLarge);
        }
        let mut buffer = vec![0_u8; plaintext.len() + AEAD_TAG_BYTES];
        let len = self.transport.write_message(plaintext, &mut buffer)?;
        buffer.truncate(len);
        Ok(buffer)
    }

    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, HandshakeError> {
        if ciphertext.len() > MAX_NOISE_MESSAGE_BYTES {
            return Err(HandshakeError::MessageTooLarge);
        }
        let mut buffer = vec![0_u8; ciphertext.len()];
        let len = self.transport.read_message(ciphertext, &mut buffer)?;
        buffer.truncate(len);
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::SEED_BYTES;

    fn identity(b: u8) -> NodeIdentity {
        NodeIdentity::from_seed(&[b; SEED_BYTES])
    }

    fn run(
        a: &NodeIdentity,
        b: &NodeIdentity,
        expect_b: Option<NodeId>,
    ) -> Result<(SecureSession, SecureSession), HandshakeError> {
        let mut init = Handshake::new(Role::Initiator, a, expect_b)?;
        let mut resp = Handshake::new(Role::Responder, b, None)?;
        let m1 = init.write_message()?;
        resp.read_message(&m1)?;
        let m2 = resp.write_message()?;
        init.read_message(&m2)?;
        let m3 = init.write_message()?;
        resp.read_message(&m3)?;
        Ok((init.finish()?, resp.finish()?))
    }

    #[test]
    fn mutual_authentication_proves_both_node_ids() {
        let (a, b) = (identity(1), identity(2));
        let (init, resp) = run(&a, &b, Some(b.node_id())).unwrap();
        assert_eq!(init.peer(), b.node_id());
        assert_eq!(resp.peer(), a.node_id());
        assert_eq!(init.peer_public_key(), b.public_key());
    }

    #[test]
    fn encrypted_traffic_round_trips_both_ways_and_hides_plaintext() {
        let (mut init, mut resp) = run(&identity(1), &identity(2), None).unwrap();
        let ct = init.encrypt(b"hello topology").unwrap();
        assert!(!ct.windows(5).any(|w| w == b"hello"));
        assert_eq!(resp.decrypt(&ct).unwrap(), b"hello topology");
        let back = resp.encrypt(b"ack").unwrap();
        assert_eq!(init.decrypt(&back).unwrap(), b"ack");
    }

    #[test]
    fn replayed_tampered_and_oversized_ciphertext_is_rejected() {
        let (mut init, mut resp) = run(&identity(1), &identity(2), None).unwrap();
        let ct = init.encrypt(b"one").unwrap();
        assert!(resp.decrypt(&ct).is_ok());
        assert!(resp.decrypt(&ct).is_err(), "replay must fail");
        let mut bad = init.encrypt(b"two").unwrap();
        bad[0] ^= 1;
        assert!(resp.decrypt(&bad).is_err());
        assert!(matches!(
            init.encrypt(&vec![0; MAX_NOISE_MESSAGE_BYTES]),
            Err(HandshakeError::MessageTooLarge)
        ));
    }

    #[test]
    fn wrong_expected_peer_is_rejected() {
        let err = run(&identity(1), &identity(2), Some(identity(3).node_id())).unwrap_err();
        assert!(matches!(err, HandshakeError::UnexpectedPeer { .. }));
    }

    #[test]
    fn identity_not_bound_to_the_noise_key_is_rejected() {
        // An attacker presents someone else's identity key with a signature over a different
        // Noise static key (here: a signature the victim made for another session).
        let victim = identity(1);
        let attacker = identity(9);
        let mut init = Handshake::new(Role::Initiator, &identity(2), None).unwrap();
        let mut resp = Handshake::new(Role::Responder, &attacker, None).unwrap();
        // Replace the responder's proof with the victim's key and a stale signature.
        resp.identity_payload[..PUBLIC_KEY_BYTES].copy_from_slice(&victim.public_key());
        let m1 = init.write_message().unwrap();
        resp.read_message(&m1).unwrap();
        let m2 = resp.write_message().unwrap();
        assert!(matches!(
            init.read_message(&m2),
            Err(HandshakeError::BadIdentity(_))
        ));
        assert!(!init.is_finished());
    }

    #[test]
    fn steps_out_of_order_and_early_finish_are_rejected() {
        let a = identity(1);
        let mut init = Handshake::new(Role::Initiator, &a, None).unwrap();
        assert!(matches!(
            init.read_message(&[0; 32]),
            Err(HandshakeError::OutOfOrder)
        ));
        let mut resp = Handshake::new(Role::Responder, &a, None).unwrap();
        assert!(matches!(
            resp.write_message(),
            Err(HandshakeError::OutOfOrder)
        ));
        assert!(matches!(init.finish(), Err(HandshakeError::OutOfOrder)));
    }

    #[test]
    fn first_message_with_a_payload_is_rejected() {
        let mut resp = Handshake::new(Role::Responder, &identity(1), None).unwrap();
        assert!(matches!(
            resp.read_message(&[0x11; 48]),
            Err(HandshakeError::MalformedIdentityPayload)
        ));
    }

    #[test]
    fn garbage_handshake_messages_error_without_panicking() {
        let a = identity(1);
        for len in [0_usize, 1, 31, 32, 33, 95, 96, 200] {
            let mut resp = Handshake::new(Role::Responder, &a, None).unwrap();
            let _ = resp.read_message(&vec![0xAB; len]);
        }
    }
}

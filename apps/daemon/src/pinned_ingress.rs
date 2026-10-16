//! P7 software-only ingress boundary for a previously verified TLS channel.
//!
//! TLS identity, mutual-authentication and exporter helpers exist in
//! `control_tls`, but they are not wired into a live peer runtime or enrollment
//! UI yet. The provided fingerprint must come from a completed
//! certificate-verified TLS handshake with private-key proof. A raw TCP
//! socket, untrusted hello, Bluetooth name or advertised fingerprint must
//! NEVER construct this gate. Production JoinNetwork remains fail-closed
//! until the real transport exists.

use blueroute_core::{CoreError, NetworkId, NodeId};
use blueroute_protocol::control::ControlEnvelope;
use blueroute_protocol::peer_pin::SpkiSha256;
use blueroute_protocol::session::ConnectionDeadline;
use blueroute_protocol::session_activity::{AuthenticatedInboundActivity, InboundActivityError};

use crate::DurablePeerTrustOperations;

#[derive(Debug)]
pub enum PinnedIngressError {
    Authorization(CoreError),
    Protocol(InboundActivityError),
    Terminal,
}

/// Opaque evidence of a cryptographically authenticated peer and the
/// exporter-bound Hello transcript. It has intentionally private fields and
/// Its production constructor consumes authenticated control-TLS evidence;
/// callers cannot fabricate it from identity claims or a fingerprint.
///
/// A public fingerprint, peer-provided NodeId, advertisement or raw socket
/// cannot forge this type through the safe API. The TLS runtime must create
/// evidence only after certificate
/// validation, key possession, independent key enrollment, correct network,
/// transcript verification and fresh exporter derivation.
///
/// Compile-fail guard: callers outside the crate cannot supply a fabricated
/// TLS exporter and identity claims in lieu of a cryptographic verifier.
///
/// ```compile_fail
/// use blueroute_core::{NetworkId, NodeId};
/// use blueroute_protocol::peer_pin::SpkiSha256;
/// use blueroute_daemon::VerifiedTlsIdentity;
///
/// let fake = VerifiedTlsIdentity {
///     network: NetworkId::from_bytes([1; 16]),
///     peer: NodeId::from_bytes([2; 16]),
///     verified_tls_key: SpkiSha256::from_verified_certificate_digest([3; 32]).unwrap(),
///     exporter_session_id: [4; 16],
/// };
/// ```
pub struct VerifiedTlsIdentity {
    network: NetworkId,
    peer: NodeId,
    verified_tls_key: SpkiSha256,
    exporter_session_id: [u8; 16],
}

impl VerifiedTlsIdentity {
    pub(crate) fn from_control_tls(
        network: NetworkId,
        peer: NodeId,
        verified_tls_key: SpkiSha256,
        exporter_session_id: [u8; 16],
    ) -> Self {
        Self {
            network,
            peer,
            verified_tls_key,
            exporter_session_id,
        }
    }
}

/// Holds the exact approved peer identity and verified channel's existing
/// post-Hello stream, but does not itself establish or verify TLS.
///
/// Every plaintext frame delivery is guarded by freshly loaded durable
/// network approval AND enrolled key pin. The daemon's trust lock covers the
/// check and processing so in-process revocation cannot interleave. An
/// authorization or framing failure irreversibly invalidates this instance.
pub struct PinnedSessionIngress<'a> {
    trust: &'a DurablePeerTrustOperations,
    network: NetworkId,
    peer: NodeId,
    verified_tls_key: SpkiSha256,
    activity: AuthenticatedInboundActivity,
    terminal: bool,
}

impl<'a> PinnedSessionIngress<'a> {
    /// Only an independently authenticated TLS/Hello runtime may call this.
    /// Construction *never* authorizes production joining by itself.
    pub fn after_verified_handshake(
        trust: &'a DurablePeerTrustOperations,
        verified_tls: VerifiedTlsIdentity,
        deadline: ConnectionDeadline,
    ) -> Result<Self, PinnedIngressError> {
        let VerifiedTlsIdentity {
            network,
            peer,
            verified_tls_key,
            exporter_session_id,
        } = verified_tls;
        trust
            .require_approved_pinned_tls_peer(network, peer, verified_tls_key)
            .map_err(PinnedIngressError::Authorization)?;
        let activity = AuthenticatedInboundActivity::after_verified_hello(
            network,
            peer,
            exporter_session_id,
            deadline,
        )
        .map_err(PinnedIngressError::Protocol)?;
        Ok(Self {
            trust,
            network,
            peer,
            verified_tls_key,
            activity,
            terminal: false,
        })
    }

    /// Recheck durable approval and monotonic time before blocking on TLS
    /// input. This does not feed parser bytes or extend liveness, preserving
    /// the clean EOF path after an authenticated Goodbye.
    pub fn check_before_read(&mut self, now_ms: u64) -> Result<(), PinnedIngressError> {
        if self.terminal {
            return Err(PinnedIngressError::Terminal);
        }
        let result = self.trust.with_approved_pinned_tls_peer(
            self.network,
            self.peer,
            self.verified_tls_key,
            || self.activity.check_before_read(now_ms),
        );
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.terminal = true;
                Err(PinnedIngressError::Protocol(error))
            }
            Err(error) => {
                self.terminal = true;
                Err(PinnedIngressError::Authorization(error))
            }
        }
    }

    pub fn receive_verified_plaintext(
        &mut self,
        plaintext: &[u8],
        now_ms: u64,
    ) -> Result<Vec<ControlEnvelope>, PinnedIngressError> {
        if self.terminal {
            return Err(PinnedIngressError::Terminal);
        }
        let result = self.trust.with_approved_pinned_tls_peer(
            self.network,
            self.peer,
            self.verified_tls_key,
            || self.activity.feed_verified_plaintext(plaintext, now_ms),
        );
        match result {
            Ok(Ok(messages)) => Ok(messages),
            Ok(Err(error)) => {
                self.terminal = true;
                Err(PinnedIngressError::Protocol(error))
            }
            Err(error) => {
                self.terminal = true;
                Err(PinnedIngressError::Authorization(error))
            }
        }
    }

    /// A graceful close still needs a current approval; teardown itself must
    /// always be performed by the caller even after revocation or failure.
    pub fn finish_at(&mut self, now_ms: u64) -> Result<(), PinnedIngressError> {
        if self.terminal {
            return Err(PinnedIngressError::Terminal);
        }
        let result = self.trust.with_approved_pinned_tls_peer(
            self.network,
            self.peer,
            self.verified_tls_key,
            || self.activity.finish_at(now_ms),
        );
        self.terminal = true;
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(PinnedIngressError::Protocol(error)),
            Err(error) => Err(PinnedIngressError::Authorization(error)),
        }
    }

    pub const fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub const fn expires_ms(&self) -> u64 {
        self.activity.expires_ms()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use blueroute_core::{
        DisplayName, ErrorKind, MembershipRegistry, MembershipState, NetworkMembership,
    };
    use blueroute_linux::NetworkMembershipStore;
    use blueroute_protocol::control::{
        CONTROL_VERSION, ControlEnvelope, ControlMessage, encode_control_frame,
    };
    use blueroute_protocol::session::{ConnectionStage, DeadlineError};

    use super::*;
    use crate::{PeerPinStore, PeerTrustOperations};

    static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "blueroute-ingress-test-{}-{}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([51; 16])
    }

    fn peer() -> NodeId {
        NodeId::from_bytes([52; 16])
    }

    fn key(value: u8) -> SpkiSha256 {
        SpkiSha256::from_verified_certificate_digest([value; 32]).unwrap()
    }

    fn deadline() -> ConnectionDeadline {
        let mut deadline = ConnectionDeadline::new(0);
        deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
        deadline.advance(ConnectionStage::Hello, 2).unwrap();
        deadline.advance(ConnectionStage::Active, 3).unwrap();
        deadline
    }

    fn setup() -> (Temp, DurablePeerTrustOperations) {
        let root = Temp::new();
        let store = NetworkMembershipStore::new(root.0.join("memberships-v1"));
        let mut network_state =
            NetworkMembership::new(network(), DisplayName::new("Simulated network").unwrap());
        network_state.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(network_state);
        store.save(&registry).unwrap();
        let trust = DurablePeerTrustOperations::with_peer_pin_store(
            store,
            PeerPinStore::new(root.0.join("peer-pins")),
        );
        (root, trust)
    }

    // This literal is for synthetic policy tests only. Production evidence is
    // created by the control-TLS Hello path and rebound to the same stream by
    // ControlTlsStream::activate before durable ingress is opened.
    fn fake_verified_tls_identity(node: NodeId, tls_key: SpkiSha256) -> VerifiedTlsIdentity {
        VerifiedTlsIdentity {
            network: network(),
            peer: node,
            verified_tls_key: tls_key,
            exporter_session_id: [53; 16],
        }
    }

    fn session<'a>(
        trust: &'a DurablePeerTrustOperations,
        tls_key: SpkiSha256,
    ) -> Result<PinnedSessionIngress<'a>, PinnedIngressError> {
        PinnedSessionIngress::after_verified_handshake(
            trust,
            fake_verified_tls_identity(peer(), tls_key),
            deadline(),
        )
    }

    fn frame(sequence: u64, message: ControlMessage, sender: NodeId) -> Vec<u8> {
        encode_control_frame(&ControlEnvelope {
            version: CONTROL_VERSION,
            network: network(),
            sender,
            session_id: [53; 16],
            sequence,
            message,
        })
        .unwrap()
    }

    #[test]
    fn neither_untrusted_approval_nor_an_unenrolled_key_opens_ingress() {
        let (_root, trust) = setup();
        assert!(matches!(
            session(&trust, key(54)),
            Err(PinnedIngressError::Authorization(_))
        ));
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        assert!(matches!(
            session(&trust, key(54)),
            Err(PinnedIngressError::Authorization(_))
        ));
        assert!(
            trust
                .enroll_verified_key(network(), peer(), key(54), key(54))
                .unwrap()
        );
        assert!(matches!(
            session(&trust, key(55)),
            Err(PinnedIngressError::Authorization(_))
        ));
        assert!(session(&trust, key(54)).is_ok());
    }

    #[test]
    fn revocation_blocks_next_frame_and_reapproval_cannot_revive_old_session() {
        let (_root, trust) = setup();
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();
        let mut active = session(&trust, key(54)).unwrap();
        assert_eq!(
            active
                .receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 10,)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(active.expires_ms(), 30_010);
        futures_lite::future::block_on(trust.forget_peer(peer())).unwrap();
        assert!(matches!(
            active.receive_verified_plaintext(&frame(4, ControlMessage::Heartbeat, peer()), 11),
            Err(PinnedIngressError::Authorization(_))
        ));
        assert!(active.is_terminal());
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        assert!(matches!(
            session(&trust, key(54)),
            Err(PinnedIngressError::Authorization(_))
        ));
        assert!(matches!(
            active.receive_verified_plaintext(&frame(5, ControlMessage::Heartbeat, peer()), 12),
            Err(PinnedIngressError::Terminal)
        ));
        // Re-enrollment can create only a FRESH, independently verified session.
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();
        assert!(session(&trust, key(54)).is_ok());
        assert!(active.is_terminal());
    }

    #[test]
    fn clock_expiry_and_bad_peer_poison_session_without_refresh() {
        let (_root, trust) = setup();
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();
        let mut active = session(&trust, key(54)).unwrap();
        assert!(matches!(
            active.receive_verified_plaintext(
                &frame(3, ControlMessage::Heartbeat, NodeId::from_bytes([99; 16])),
                10
            ),
            Err(PinnedIngressError::Protocol(_))
        ));
        assert!(active.is_terminal());
        assert!(matches!(
            active.receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 11),
            Err(PinnedIngressError::Terminal)
        ));

        let mut expired = session(&trust, key(54)).unwrap();
        assert!(matches!(
            expired
                .receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 30_003),
            Err(PinnedIngressError::Protocol(
                InboundActivityError::Deadline(DeadlineError::Expired)
            ))
        ));
        assert!(expired.is_terminal());
    }

    #[test]
    fn pre_read_authorization_preserves_goodbye_eof_and_revocation_is_terminal() {
        let (_root, trust) = setup();
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();

        let mut closing = session(&trust, key(54)).unwrap();
        closing
            .receive_verified_plaintext(&frame(3, ControlMessage::Goodbye, peer()), 10)
            .unwrap();
        assert!(closing.check_before_read(11).is_ok());
        assert!(closing.finish_at(11).is_ok());

        let mut revoked = session(&trust, key(54)).unwrap();
        futures_lite::future::block_on(trust.forget_peer(peer())).unwrap();
        assert!(matches!(
            revoked.check_before_read(4),
            Err(PinnedIngressError::Authorization(_))
        ));
        assert!(revoked.is_terminal());
        assert!(matches!(
            revoked.check_before_read(5),
            Err(PinnedIngressError::Terminal)
        ));
    }

    #[test]
    fn pre_read_clock_check_never_renews_idle_and_expires_terminally() {
        let (_root, trust) = setup();
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();

        let mut pending = session(&trust, key(54)).unwrap();
        assert_eq!(pending.expires_ms(), 30_003);
        assert!(pending.check_before_read(30_002).is_ok());
        assert_eq!(pending.expires_ms(), 30_003);
        assert!(matches!(
            pending.check_before_read(30_003),
            Err(PinnedIngressError::Protocol(
                InboundActivityError::Deadline(DeadlineError::Expired)
            ))
        ));
        assert!(pending.is_terminal());

        let mut regressed = session(&trust, key(54)).unwrap();
        regressed
            .receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 20)
            .unwrap();
        assert_eq!(regressed.expires_ms(), 30_020);
        assert!(matches!(
            regressed.check_before_read(19),
            Err(PinnedIngressError::Protocol(
                InboundActivityError::Deadline(DeadlineError::ClockRegression)
            ))
        ));
        assert!(regressed.is_terminal());
    }

    #[test]
    fn revocation_refuses_even_graceful_close_and_teardown_stays_terminal() {
        let (_root, trust) = setup();
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();
        let mut active = session(&trust, key(54)).unwrap();
        active
            .receive_verified_plaintext(&frame(3, ControlMessage::Goodbye, peer()), 10)
            .unwrap();
        futures_lite::future::block_on(trust.forget_peer(peer())).unwrap();
        match active.finish_at(11) {
            Err(PinnedIngressError::Authorization(error)) => {
                assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
            }
            _ => panic!("revoked peer must not close as approved"),
        }
        assert!(active.is_terminal());
        assert!(matches!(
            active.finish_at(12),
            Err(PinnedIngressError::Terminal)
        ));
    }

    #[test]
    fn corrupt_persistent_pin_state_poisoning_cannot_be_recovered_in_place() {
        let (root, trust) = setup();
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();
        let mut active = session(&trust, key(54)).unwrap();
        let persisted = PeerPinStore::new(root.0.join("peer-pins"));
        let original = persisted.load().unwrap();
        fs::write(
            root.0.join("peer-pins").join("peer-key-pins-v1"),
            b"invalid-state",
        )
        .unwrap();

        match active.receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 10) {
            Err(PinnedIngressError::Authorization(error)) => {
                assert_eq!(error.kind(), ErrorKind::PersistenceError);
            }
            _ => panic!("corrupt durable pin state must reject the complete frame"),
        }
        assert!(active.is_terminal());
        assert_eq!(active.expires_ms(), 30_003);
        // Repairing the file must not revive the old logical TLS session.
        persisted.save(&original).unwrap();
        assert!(matches!(
            active.receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 11),
            Err(PinnedIngressError::Terminal)
        ));
        assert!(session(&trust, key(54)).is_ok());
    }

    #[test]
    fn two_enrolled_peers_have_independent_pin_and_session_scope() {
        let (_root, trust) = setup();
        let second = NodeId::from_bytes([61; 16]);
        futures_lite::future::block_on(trust.trust_peer(peer())).unwrap();
        futures_lite::future::block_on(trust.trust_peer(second)).unwrap();
        trust
            .enroll_verified_key(network(), peer(), key(54), key(54))
            .unwrap();
        trust
            .enroll_verified_key(network(), second, key(55), key(55))
            .unwrap();
        let mut first = session(&trust, key(54)).unwrap();
        let mut other = PinnedSessionIngress::after_verified_handshake(
            &trust,
            fake_verified_tls_identity(second, key(55)),
            deadline(),
        )
        .unwrap();

        assert_eq!(
            first
                .receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 10)
                .unwrap()
                .len(),
            1
        );
        // A second peer's perfectly framed message cannot be interpreted
        // as the first peer's authenticated control envelope.
        assert!(matches!(
            other.receive_verified_plaintext(&frame(3, ControlMessage::Heartbeat, peer()), 11),
            Err(PinnedIngressError::Protocol(_))
        ));
        assert!(other.is_terminal());
        assert_eq!(
            first
                .receive_verified_plaintext(&frame(4, ControlMessage::Heartbeat, peer()), 12)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(first.expires_ms(), 30_012);
        // Neither a different key nor another NodeId's approval substitutes
        // for an independent pinned identity.
        assert!(matches!(
            session(&trust, key(55)),
            Err(PinnedIngressError::Authorization(_))
        ));
    }
}

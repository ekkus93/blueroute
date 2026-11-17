//! Pinned TLS 1.3 identities for BlueRoute's inter-node control channel.
//!
//! TLS uses standard X.509 certificates and the rustls ring provider. The
//! certificate's SubjectPublicKeyInfo is pinned to a separately approved
//! `(NetworkId, NodeId)` by the peer-pin policy. A TLS handshake must still
//! prove possession of the matching private key. This module does not enroll
//! peers or activate JoinNetwork.

mod certificate;
mod enrollment;
mod identity;
mod session;
mod transport;

pub use self::certificate::{
    ControlTlsCertificateCandidate, PinnedControlTlsCertificate, PinnedControlTlsPeer,
};
pub use self::enrollment::{exchange_control_tls_candidate, exchange_control_tls_certificate};
pub use self::identity::{ControlTlsIdentityStore, LocalControlTlsIdentity};
pub use self::session::{
    ActiveControlSession, AuthenticatedControlPeer, ControlSessionError, ControlSessionRead,
    export_control_session_id,
};
pub use self::transport::{
    ControlTlsStream, VerifiedControlTlsPeer, accept_pinned_control_tls,
    connect_pinned_control_tls, verify_control_tls_peer,
};
use blueroute_core::{NetworkId, NodeId};
use rustls::SupportedProtocolVersion;
use std::fmt;
use std::time::Duration;

static TLS13_ONLY: [&SupportedProtocolVersion; 1] = [&rustls::version::TLS13];

const MAX_CERTIFICATE_BYTES: usize = 16 * 1024;

const MAX_PRIVATE_KEY_BYTES: usize = 16 * 1024;

// A certificate preface is unauthenticated discovery evidence, not a reusable
// enrollment authorization. A fresh candidate must be acquired after this age.
const MAX_ENROLLMENT_CANDIDATE_AGE: Duration = Duration::from_secs(300);

const CERTIFICATE_PREFACE: &[u8] = b"BLUEROUTE-CONTROL-CERT-V1\0";

const IDENTITY_FILE: &str = "control-tls-identity-v1";

const IDENTITY_HEADER: &[u8] = b"BLUEROUTE_CONTROL_TLS_IDENTITY_V1\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlTlsError {
    InvalidIdentity,
    InvalidKey,
    InvalidCertificate,
    InvalidConfiguration,
    FingerprintMismatch,
    CertificateIdentityMismatch,
    KeyCertificateMismatch,
    HandshakeNotComplete,
    ExporterUnavailable,
    MissingPeerCertificate,
    HandshakeDeadlineExpired,
    HandshakeFailed,
    HelloExchangeFailed,
    RandomnessUnavailable,
    ExporterMismatch,
    CertificateExchangeFailed,
    CertificateExchangeDeadlineExpired,
    EnrollmentCandidateExpired,
}

impl fmt::Display for ControlTlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidIdentity => "invalid BlueRoute TLS identity",
            Self::InvalidKey => "invalid BlueRoute TLS private key",
            Self::InvalidCertificate => "invalid BlueRoute TLS certificate",
            Self::InvalidConfiguration => "failed to configure TLS 1.3 mutual authentication",
            Self::FingerprintMismatch => "peer TLS certificate does not match its enrolled key",
            Self::CertificateIdentityMismatch => {
                "peer TLS certificate does not name its enrolled BlueRoute node"
            }
            Self::KeyCertificateMismatch => "BlueRoute TLS certificate and key do not match",
            Self::HandshakeNotComplete => "TLS handshake is not complete",
            Self::ExporterUnavailable => "TLS control session exporter is unavailable",
            Self::MissingPeerCertificate => "TLS peer did not present a certificate",
            Self::HandshakeDeadlineExpired => "TLS handshake deadline expired",
            Self::HandshakeFailed => "TLS handshake failed",
            Self::HelloExchangeFailed => "authenticated TLS Hello exchange failed",
            Self::RandomnessUnavailable => "secure control challenge generation failed",
            Self::ExporterMismatch => "authenticated Hello belongs to a different TLS channel",
            Self::CertificateExchangeFailed => "control certificate exchange failed",
            Self::CertificateExchangeDeadlineExpired => {
                "control certificate exchange deadline expired"
            }
            Self::EnrollmentCandidateExpired => {
                "untrusted enrollment candidate has expired; discover the peer again"
            }
        })
    }
}

impl std::error::Error for ControlTlsError {}

fn is_zero_node(node: NodeId) -> bool {
    node == NodeId::from_bytes([0; 16])
}

fn is_zero_network(network: NetworkId) -> bool {
    network == NetworkId::from_bytes([0; 16])
}

#[cfg(test)]
mod test_support {
    use crate::{DurablePeerTrustOperations, PeerPinStore, PeerTrustOperations};
    use blueroute_core::{DisplayName, MembershipRegistry, MembershipState, NetworkMembership};
    use blueroute_linux::NetworkMembershipStore;

    use std::fs;

    use std::net::{TcpListener, TcpStream};

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use blueroute_protocol::control::HelloFacts;
    use blueroute_protocol::session::ConnectionDeadline;

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    pub(super) struct TempDirectory(pub(super) PathBuf);

    impl TempDirectory {
        pub(super) fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("blueroute-control-tls-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(super) fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    pub(super) fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    pub(super) fn make_tls_pair() -> (LocalControlTlsIdentity, LocalControlTlsIdentity) {
        let client = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
        (client, server)
    }

    pub(super) fn stream_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    pub(super) fn hello_facts(version: &str) -> HelloFacts {
        HelloFacts {
            software_version: version.to_owned(),
            capabilities: blueroute_core::NodeCapabilities::default(),
            segment: None,
            direct_neighbors: Vec::new(),
        }
    }

    pub(super) fn trust_for_peer(
        directory: &std::path::Path,
        verified_peer: &VerifiedControlTlsPeer,
    ) -> DurablePeerTrustOperations {
        let memberships = NetworkMembershipStore::new(directory.join("memberships-v1"));
        let mut member =
            NetworkMembership::new(network(9), DisplayName::new("TLS test network").unwrap());
        member.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(member);
        memberships.save(&registry).unwrap();
        let trust = DurablePeerTrustOperations::with_peer_pin_store(
            memberships,
            PeerPinStore::new(directory.join("peer-pins")),
        );
        futures_lite::future::block_on(trust.trust_peer(verified_peer.node())).unwrap();
        trust
            .enroll_key_after_independent_confirmation(verified_peer, verified_peer.fingerprint())
            .unwrap();
        trust
    }

    pub(super) fn active_deadline() -> ConnectionDeadline {
        use blueroute_protocol::session::ConnectionStage;

        let mut deadline = ConnectionDeadline::new(0);
        deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
        deadline.advance(ConnectionStage::Hello, 2).unwrap();
        deadline.advance(ConnectionStage::Active, 3).unwrap();
        deadline
    }
}

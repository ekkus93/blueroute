//! Explicitly confirmed first-key enrollment after real mutual TLS proof.
//!
//! The candidate preface is untrusted. This primitive does not obtain human
//! confirmation: an independent operator-facing ceremony must supply the
//! confirmed fingerprint. It cannot activate membership or production join.

use std::net::{Shutdown, TcpStream};
use std::time::Instant;

use blueroute_core::{CoreError, ErrorKind};
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::peer_pin::SpkiSha256;

use crate::{
    ControlTlsCertificateCandidate, DurablePeerTrustOperations, LocalControlTlsIdentity,
    connect_pinned_control_tls,
};

fn authentication_error(message: &'static str, detail: impl std::fmt::Display) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::AuthenticationFailed, message, detail.to_string())
}

/// Bind independently confirmed candidate material to a new TLS 1.3 channel
/// and its exporter-bound Hello/HelloAck before persisting a peer key pin.
///
/// The caller must have shown the candidate NodeId and SPKI fingerprint to
/// the user and obtained an independent out-of-band comparison. A preface or
/// Bluetooth pairing alone never counts as approval. The peer must already
/// have explicit durable P6-003 membership approval for the same network.
///
/// This consumes a *new* TCP socket after candidate discovery. It authenticates
/// the candidate again on that socket, then persists only the cryptographically
/// verified fingerprint. The socket closes on both success and failure.
/// No session is registered and production JoinNetwork remains fail-closed.
pub fn enroll_confirmed_control_candidate(
    socket: TcpStream,
    local: &LocalControlTlsIdentity,
    candidate: ControlTlsCertificateCandidate,
    independently_confirmed: SpkiSha256,
    trust: &DurablePeerTrustOperations,
    local_facts: HelloFacts,
    deadline: Instant,
) -> Result<bool, CoreError> {
    let network = candidate.network();
    let node = candidate.node();
    if let Err(error) = trust.require_peer_approved(network, node) {
        let _ = socket.shutdown(Shutdown::Both);
        return Err(error);
    }

    let certificate = match candidate.confirm_fingerprint(independently_confirmed) {
        Ok(certificate) => certificate,
        Err(error) => {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(authentication_error(
                "candidate fingerprint does not match independent confirmation",
                error,
            ));
        }
    };
    let (mut stream, observed_certificate) =
        connect_pinned_control_tls(socket, local, network, certificate.peer_pin(), deadline)
            .map_err(|error| authentication_error("confirmed peer TLS proof failed", error))?;

    // The authenticated result is opaque and can only come from the current
    // rustls connection, not a caller-supplied NodeId or fingerprint claim.
    let verified = stream.exchange_authenticated_hello(
        network,
        local,
        &observed_certificate,
        local_facts,
        deadline,
    );
    let result = match verified {
        Ok(verified) => trust
            .enroll_key_after_independent_confirmation(verified.peer(), independently_confirmed),
        Err(error) => Err(authentication_error(
            "confirmed peer TLS Hello exchange failed",
            error,
        )),
    };
    stream.close();
    result
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    use blueroute_core::{
        DisplayName, MembershipRegistry, MembershipState, NetworkId, NetworkMembership,
        NodeCapabilities, NodeId,
    };
    use blueroute_linux::NetworkMembershipStore;

    use super::*;
    use crate::{
        PeerPinStore, PeerTrustOperations, PinnedControlTlsPeer, accept_pinned_control_tls,
        exchange_control_tls_candidate,
    };

    static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

    fn network() -> NetworkId {
        NetworkId::from_bytes([17; 16])
    }

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn facts() -> HelloFacts {
        HelloFacts {
            software_version: "enrollment-bootstrap-test".to_owned(),
            capabilities: NodeCapabilities::default(),
            segment: None,
            direct_neighbors: Vec::new(),
        }
    }

    #[test]
    fn first_key_is_persisted_only_after_approval_confirmation_tls_and_hello() {
        let root = std::env::temp_dir().join(format!(
            "blueroute-enrollment-bootstrap-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        let store = NetworkMembershipStore::new(root.join("membership"));
        let mut membership =
            NetworkMembership::new(network(), DisplayName::new("Enrollment test").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        store.save(&registry).unwrap();
        let trust = DurablePeerTrustOperations::with_peer_pin_store(
            store,
            PeerPinStore::new(root.join("pins")),
        );

        let client = LocalControlTlsIdentity::generate(node(2)).unwrap();
        let server = LocalControlTlsIdentity::generate(node(3)).unwrap();
        let expected_key = server.fingerprint();
        let client_pin =
            PinnedControlTlsPeer::new(network(), client.node(), client.fingerprint()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server_thread = thread::spawn(move || {
            // Stage 1 discovers only untrusted certificate material.
            let (first, _) = listener.accept().unwrap();
            let _untrusted = exchange_control_tls_candidate(
                first,
                &server,
                network(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();

            // Unapproved NodeId and wrong independent comparison MUST fail
            // before sending TLS or even a second certificate preface.
            for _ in 0..2 {
                let (mut denied, _) = listener.accept().unwrap();
                denied
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                assert_eq!(denied.read(&mut [0_u8; 1]).unwrap(), 0);
            }

            // Only the third attempt may reach mutual TLS and Hello.
            let (accepted, _) = listener.accept().unwrap();
            let (mut stream, cert) = accept_pinned_control_tls(
                accepted,
                &server,
                network(),
                &[client_pin],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
            let authenticated = stream
                .exchange_authenticated_hello(
                    network(),
                    &server,
                    &cert,
                    facts(),
                    Instant::now() + Duration::from_secs(5),
                )
                .unwrap();
            assert_eq!(authenticated.peer().node(), client_pin.node());
            stream.close();
        });

        let candidate = exchange_control_tls_candidate(
            TcpStream::connect(addr).unwrap(),
            &client,
            network(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(candidate.node(), node(3));
        assert_eq!(candidate.fingerprint(), expected_key);
        assert_eq!(
            enroll_confirmed_control_candidate(
                TcpStream::connect(addr).unwrap(),
                &client,
                candidate.clone(),
                expected_key,
                &trust,
                facts(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap_err()
            .kind(),
            ErrorKind::AuthenticationFailed
        );

        futures_lite::future::block_on(trust.trust_peer(node(3))).unwrap();
        let wrong_key = SpkiSha256::from_verified_certificate_digest([42; 32]).unwrap();
        assert_eq!(
            enroll_confirmed_control_candidate(
                TcpStream::connect(addr).unwrap(),
                &client,
                candidate.clone(),
                wrong_key,
                &trust,
                facts(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap_err()
            .kind(),
            ErrorKind::AuthenticationFailed
        );
        assert!(trust.approved_control_pins(network()).unwrap().is_empty());

        assert_eq!(
            enroll_confirmed_control_candidate(
                TcpStream::connect(addr).unwrap(),
                &client,
                candidate,
                expected_key,
                &trust,
                facts(),
                Instant::now() + Duration::from_secs(5),
            ),
            Ok(true)
        );
        server_thread.join().unwrap();
        assert_eq!(
            trust.approved_control_pins(network()).unwrap(),
            vec![PinnedControlTlsPeer::new(network(), node(3), expected_key).unwrap()]
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn key_rotation_between_candidate_discovery_and_tls_never_enrolls() {
        // Two distinct certificates can claim the same canonical NodeId.
        // The independently confirmed first certificate is not proof that
        // the second socket still possesses that same private key.
        let root = std::env::temp_dir().join(format!(
            "blueroute-enrollment-key-rotation-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        let store = NetworkMembershipStore::new(root.join("membership"));
        let mut membership =
            NetworkMembership::new(network(), DisplayName::new("Rotation test").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        store.save(&registry).unwrap();
        let trust = DurablePeerTrustOperations::with_peer_pin_store(
            store,
            PeerPinStore::new(root.join("pins")),
        );
        futures_lite::future::block_on(trust.trust_peer(node(3))).unwrap();

        let client = LocalControlTlsIdentity::generate(node(2)).unwrap();
        let advertised = LocalControlTlsIdentity::generate(node(3)).unwrap();
        let rotated = LocalControlTlsIdentity::generate(node(3)).unwrap();
        assert_ne!(advertised.fingerprint(), rotated.fingerprint());
        let confirmed = advertised.fingerprint();
        let client_pin =
            PinnedControlTlsPeer::new(network(), client.node(), client.fingerprint()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server_thread = thread::spawn(move || {
            let (first, _) = listener.accept().unwrap();
            exchange_control_tls_candidate(
                first,
                &advertised,
                network(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();

            let (second, _) = listener.accept().unwrap();
            // The rotated server can exchange candidate prefaces, but the
            // client closes before unapproved TLS can become active.
            assert!(
                accept_pinned_control_tls(
                    second,
                    &rotated,
                    network(),
                    &[client_pin],
                    Instant::now() + Duration::from_secs(5),
                )
                .is_err()
            );
        });

        let candidate = exchange_control_tls_candidate(
            TcpStream::connect(addr).unwrap(),
            &client,
            network(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(candidate.fingerprint(), confirmed);
        assert_eq!(
            enroll_confirmed_control_candidate(
                TcpStream::connect(addr).unwrap(),
                &client,
                candidate,
                confirmed,
                &trust,
                facts(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap_err()
            .kind(),
            ErrorKind::AuthenticationFailed
        );
        server_thread.join().unwrap();
        assert!(trust.approved_control_pins(network()).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}

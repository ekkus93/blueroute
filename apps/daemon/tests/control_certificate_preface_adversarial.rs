//! Adversarial TCP preface cases for candidate key enrollment.
//!
//! This is a parser-boundary test, not user approval, TLS peer proof,
//! daemon PAN integration, or production JoinNetwork acceptance.

use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use blueroute_core::{NetworkId, NodeId};
use blueroute_daemon::{
    ControlTlsError, LocalControlTlsIdentity, PinnedControlTlsPeer, exchange_control_tls_candidate,
    exchange_control_tls_certificate,
};

const PREFACE: &[u8] = b"BLUEROUTE-CONTROL-CERT-V1\0";
const CERT_LIMIT: u32 = 16 * 1024;

fn node(byte: u8) -> NodeId {
    NodeId::from_bytes([byte; 16])
}

fn network() -> NetworkId {
    NetworkId::from_bytes([9; 16])
}

/// The adversarial peer never receives trusted authority from this exchange.
fn reject_candidate(payload: &[u8], expected: ControlTlsError) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let server_thread = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        exchange_control_tls_candidate(
            socket,
            &server,
            network(),
            Instant::now() + Duration::from_secs(2),
        )
    });

    let mut attacker = TcpStream::connect(address).unwrap();
    attacker.write_all(payload).unwrap();
    attacker.shutdown(Shutdown::Write).unwrap();
    assert_eq!(server_thread.join().unwrap().unwrap_err(), expected);
}

fn reject_stalled_candidate(payload: &[u8]) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let server_thread = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        exchange_control_tls_candidate(
            socket,
            &server,
            network(),
            Instant::now() + Duration::from_millis(300),
        )
    });

    // Keep the write side open: this tests the absolute deadline rather
    // than the early-EOF rejection exercised by reject_candidate.
    let mut attacker = TcpStream::connect(address).unwrap();
    attacker.write_all(payload).unwrap();
    assert_eq!(
        server_thread.join().unwrap().unwrap_err(),
        ControlTlsError::CertificateExchangeDeadlineExpired,
    );
}

#[test]
fn certificate_candidate_rejects_bad_magic() {
    let mut payload = PREFACE.to_vec();
    payload[0] ^= 1;
    payload.extend_from_slice(&1_u32.to_be_bytes());
    payload.push(0);
    reject_candidate(&payload, ControlTlsError::CertificateExchangeFailed);
}

#[test]
fn certificate_candidate_rejects_truncated_preface_header() {
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&[0, 0]);
    reject_candidate(&payload, ControlTlsError::CertificateExchangeFailed);
}

#[test]
fn certificate_candidate_rejects_oversized_declared_certificate_before_body() {
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&(CERT_LIMIT + 1).to_be_bytes());
    reject_candidate(&payload, ControlTlsError::InvalidCertificate);
}

#[test]
fn certificate_candidate_rejects_zero_length_certificate() {
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&0_u32.to_be_bytes());
    reject_candidate(&payload, ControlTlsError::InvalidCertificate);
}

#[test]
fn certificate_candidate_rejects_truncated_declared_body() {
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&16_u32.to_be_bytes());
    payload.extend_from_slice(&[0x30, 0x02, 0x01, 0x01]);
    // EOF before the declared body is complete must not create a candidate.
    reject_candidate(&payload, ControlTlsError::CertificateExchangeFailed);
}

#[test]
fn certificate_candidate_rejects_complete_malformed_der() {
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&4_u32.to_be_bytes());
    payload.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
    // A complete length-delimited body is still untrusted certificate input.
    reject_candidate(&payload, ControlTlsError::InvalidCertificate);
}

#[test]
fn certificate_candidate_rejects_its_own_node_even_with_valid_der() {
    // A peer can echo a syntactically valid certificate, but this must never
    // become an enrollable identity for the local daemon itself.
    let reflected = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let certificate = reflected.certificate().as_ref();
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&(certificate.len() as u32).to_be_bytes());
    payload.extend_from_slice(certificate);
    reject_candidate(&payload, ControlTlsError::InvalidIdentity);
}

#[test]
fn zero_network_candidate_is_rejected_before_any_certificate_exchange() {
    use std::io::Read;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let server_thread = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        exchange_control_tls_candidate(
            socket,
            &server,
            NetworkId::from_bytes([0; 16]),
            Instant::now() + Duration::from_secs(2),
        )
    });

    let mut peer = TcpStream::connect(address).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert_eq!(
        server_thread.join().unwrap().unwrap_err(),
        ControlTlsError::InvalidIdentity,
    );
    // No cleartext candidate certificate should be leaked on invalid scope.
    let mut first_byte = [0; 1];
    assert_eq!(peer.read(&mut first_byte).unwrap(), 0);
}

#[test]
fn invalid_pin_allowlists_close_borrowed_socket_without_disclosing_certificate() {
    use std::io::Read;

    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let remote = LocalControlTlsIdentity::generate(node(3)).unwrap();
    let valid = PinnedControlTlsPeer::new(network(), remote.node(), remote.fingerprint()).unwrap();
    let foreign = PinnedControlTlsPeer::new(
        NetworkId::from_bytes([7; 16]),
        remote.node(),
        remote.fingerprint(),
    )
    .unwrap();
    // An alias NodeId with the same key and the server's own NodeId
    // must never enter the trusted pre-TLS certificate allowlist.
    let alias = PinnedControlTlsPeer::new(network(), node(4), remote.fingerprint()).unwrap();
    let self_peer =
        PinnedControlTlsPeer::new(network(), server.node(), server.fingerprint()).unwrap();

    // No approved pins, zero network, duplicate peer, and foreign-network
    // pins must all fail before sending the daemon's cleartext certificate.
    for (scope, pins) in [
        (network(), vec![]),
        (NetworkId::from_bytes([0; 16]), vec![valid]),
        (network(), vec![valid, valid]),
        (network(), vec![valid, alias]),
        (network(), vec![self_peer]),
        (network(), vec![foreign]),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (mut accepted, _) = listener.accept().unwrap();
        assert_eq!(
            exchange_control_tls_certificate(
                &mut accepted,
                &server,
                scope,
                &pins,
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap_err(),
            ControlTlsError::InvalidIdentity,
        );
        let mut received = [0; 1];
        assert_eq!(peer.read(&mut received).unwrap(), 0);
    }
}

#[test]
fn approved_certificate_exchange_restores_borrowed_socket_timeouts() {
    // The pre-TLS parser temporarily installs absolute-deadline read/write
    // timeouts. On success, it must return the borrowed socket to its caller
    // with the original settings, rather than weakening later TLS I/O policy.
    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let client = LocalControlTlsIdentity::generate(node(3)).unwrap();
    let server_pin =
        PinnedControlTlsPeer::new(network(), server.node(), server.fingerprint()).unwrap();
    let client_pin =
        PinnedControlTlsPeer::new(network(), client.node(), client.fingerprint()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_thread = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let original_read = Duration::from_millis(1300);
        let original_write = Duration::from_millis(1700);
        socket.set_read_timeout(Some(original_read)).unwrap();
        socket.set_write_timeout(Some(original_write)).unwrap();
        let accepted = exchange_control_tls_certificate(
            &mut socket,
            &server,
            network(),
            &[client_pin],
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(accepted.fingerprint(), client_pin.fingerprint());
        assert_eq!(socket.read_timeout().unwrap(), Some(original_read));
        assert_eq!(socket.write_timeout().unwrap(), Some(original_write));
    });
    let mut socket = TcpStream::connect(address).unwrap();
    let original_read = Duration::from_millis(900);
    let original_write = Duration::from_millis(1500);
    socket.set_read_timeout(Some(original_read)).unwrap();
    socket.set_write_timeout(Some(original_write)).unwrap();
    let accepted = exchange_control_tls_certificate(
        &mut socket,
        &client,
        network(),
        &[server_pin],
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(accepted.fingerprint(), server_pin.fingerprint());
    assert_eq!(socket.read_timeout().unwrap(), Some(original_read));
    assert_eq!(socket.write_timeout().unwrap(), Some(original_write));
    server_thread.join().unwrap();
}

#[test]
fn certificate_candidate_rejects_uppercase_hex_alias_in_node_san() {
    // General NodeId input accepts uppercase hex, but the signed DNS SAN
    // is a canonical protocol identity and must not accept textual aliases.
    let upper = NodeId::from_bytes([0xab; 16]).to_string().to_uppercase();
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec![format!("node-{upper}")]).unwrap();
    let certificate = params.self_signed(&key).unwrap();
    let der = certificate.der().as_ref();
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&(der.len() as u32).to_be_bytes());
    payload.extend_from_slice(der);
    reject_candidate(&payload, ControlTlsError::CertificateIdentityMismatch);
}

#[test]
fn candidate_certificate_refuses_missing_node_san() {
    // A valid signed X.509 leaf without any DNS identity is not an
    // enrollable logical peer, irrespective of its possession of a key.
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    let certificate = params.self_signed(&key).unwrap();
    let der = certificate.der().as_ref();
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&(der.len() as u32).to_be_bytes());
    payload.extend_from_slice(der);
    reject_candidate(&payload, ControlTlsError::CertificateIdentityMismatch);
}

#[test]
fn candidate_certificate_refuses_multiple_node_sans() {
    // Never select a convenient peer identity from an ambiguous signed
    // certificate carrying multiple otherwise valid node names.
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec![
        format!("node-{}", node(3)),
        format!("node-{}", node(4)),
    ])
    .unwrap();
    let certificate = params.self_signed(&key).unwrap();
    let der = certificate.der().as_ref();
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&(der.len() as u32).to_be_bytes());
    payload.extend_from_slice(der);
    reject_candidate(&payload, ControlTlsError::CertificateIdentityMismatch);
}

#[test]
fn candidate_certificate_refuses_zero_node_san() {
    // A syntactically valid signed certificate for the all-zero NodeId
    // must not become an enrollment candidate or a future TLS pin.
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec![format!("node-{}", node(0))]).unwrap();
    let certificate = params.self_signed(&key).unwrap();
    let der = certificate.der().as_ref();
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&(der.len() as u32).to_be_bytes());
    payload.extend_from_slice(der);
    reject_candidate(&payload, ControlTlsError::InvalidIdentity);
}

#[test]
fn stalled_certificate_preface_expires_whole_exchange_deadline() {
    reject_stalled_candidate(&[]);
}

#[test]
fn stalled_partial_certificate_header_expires_whole_exchange_deadline() {
    // The peer sends a valid magic but only half the four-byte length.
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&[0, 0]);
    reject_stalled_candidate(&payload);
}

#[test]
fn stalled_partial_certificate_body_expires_whole_exchange_deadline() {
    // The header declares a body, but the peer never sends its remainder.
    let mut payload = PREFACE.to_vec();
    payload.extend_from_slice(&16_u32.to_be_bytes());
    payload.extend_from_slice(&[0x30, 0x02, 0x01, 0x01]);
    reject_stalled_candidate(&payload);
}

/// Exercise valid X.509 identity material at the *approved* pre-TLS boundary.
/// A parseable certificate is not sufficient to enter mutual TLS: its claimed
/// NodeId and SPKI must match the exact independently enrolled allowlist.
fn reject_unapproved_certificate_after_preface(
    presented: &LocalControlTlsIdentity,
    approved: PinnedControlTlsPeer,
    expected: ControlTlsError,
) {
    use std::io::Read;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let server_thread = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        exchange_control_tls_certificate(
            &mut socket,
            &server,
            network(),
            &[approved],
            Instant::now() + Duration::from_secs(3),
        )
    });

    let mut peer = TcpStream::connect(address).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let cert = presented.certificate().as_ref();
    let mut preface = PREFACE.to_vec();
    preface.extend_from_slice(&(cert.len() as u32).to_be_bytes());
    preface.extend_from_slice(cert);
    peer.write_all(&preface).unwrap();

    // The protocol deliberately exchanges public certificates before checking
    // the allowlist. The failure must then close the socket instead of
    // allowing a client to continue into TLS on an unapproved identity.
    let mut response = Vec::new();
    peer.read_to_end(&mut response).unwrap();
    assert!(response.starts_with(PREFACE));
    assert!(response.len() > PREFACE.len() + 4);
    assert_eq!(server_thread.join().unwrap().unwrap_err(), expected);
}

#[test]
fn approved_preface_rejects_valid_certificate_for_unknown_node_and_closes_socket() {
    let presented = LocalControlTlsIdentity::generate(node(3)).unwrap();
    let known = LocalControlTlsIdentity::generate(node(4)).unwrap();
    let approved = PinnedControlTlsPeer::new(network(), known.node(), known.fingerprint()).unwrap();
    reject_unapproved_certificate_after_preface(
        &presented,
        approved,
        ControlTlsError::CertificateIdentityMismatch,
    );
}

#[test]
fn approved_preface_rejects_valid_node_with_wrong_key_and_closes_socket() {
    let presented = LocalControlTlsIdentity::generate(node(3)).unwrap();
    let other_key = LocalControlTlsIdentity::generate(node(4)).unwrap();
    // Same approved NodeId, but a different public key: NodeId-only trust must
    // never promote an unpinned certificate into a session.
    let approved =
        PinnedControlTlsPeer::new(network(), presented.node(), other_key.fingerprint()).unwrap();
    reject_unapproved_certificate_after_preface(
        &presented,
        approved,
        ControlTlsError::FingerprintMismatch,
    );
}

//! Adversarial TCP preface cases for candidate key enrollment.
//!
//! This is a parser-boundary test, not user approval, TLS peer proof,
//! daemon PAN integration, or production JoinNetwork acceptance.

use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use blueroute_core::{NetworkId, NodeId};
use blueroute_daemon::{ControlTlsError, LocalControlTlsIdentity, exchange_control_tls_candidate};

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

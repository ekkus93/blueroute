//! The bounded certificate-preface exchange used before enrollment. It yields unauthenticated candidates only; it never enrolls a peer.

use super::certificate::{
    ControlTlsCertificateCandidate, PinnedControlTlsCertificate, PinnedControlTlsPeer,
    certificate_fingerprint, certificate_node_id,
};
use super::identity::LocalControlTlsIdentity;
use super::{
    CERTIFICATE_PREFACE, ControlTlsError, MAX_CERTIFICATE_BYTES, is_zero_network, is_zero_node,
};
use blueroute_core::NetworkId;
use rustls::pki_types::CertificateDer;
use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Instant;

/// Exchange bounded candidate certificates over the dedicated control TCP
/// socket, then match the remote certificate's NodeId and SPKI against the
/// caller's already approved pin set. Only the resulting exact certificate is
/// suitable for the subsequent rustls trust configuration.
pub fn exchange_control_tls_certificate(
    socket: &mut TcpStream,
    local: &LocalControlTlsIdentity,
    network: NetworkId,
    allowed_peers: &[PinnedControlTlsPeer],
    deadline: Instant,
) -> Result<PinnedControlTlsCertificate, ControlTlsError> {
    // Reject ambiguous per-network key mappings before exchanging even a
    // public certificate. Ordered sets avoid quadratic rescanning when
    // many independently enrolled clients share a NAP.
    let mut nodes = BTreeSet::new();
    let mut fingerprints = BTreeSet::new();
    if is_zero_network(network)
        || allowed_peers.is_empty()
        || allowed_peers.iter().any(|peer| {
            peer.network != network
                || peer.node == local.node
                || !nodes.insert(peer.node)
                || !fingerprints.insert(peer.fingerprint)
        })
    {
        // This public borrowed-socket API must fail closed, too: callers
        // cannot continue using an unauthenticated stream after bad policy.
        let _ = socket.shutdown(Shutdown::Both);
        return Err(ControlTlsError::InvalidIdentity);
    }
    let original_read_timeout = socket
        .read_timeout()
        .map_err(|_| ControlTlsError::CertificateExchangeFailed)?;
    let original_write_timeout = socket
        .write_timeout()
        .map_err(|_| ControlTlsError::CertificateExchangeFailed)?;

    let candidate = match exchange_control_tls_candidate_inner(socket, local, network, deadline) {
        Ok(candidate) => candidate,
        Err(error) => {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(error);
        }
    };
    let Some(expected) = allowed_peers
        .iter()
        .find(|peer| peer.node == candidate.node)
    else {
        let _ = socket.shutdown(Shutdown::Both);
        return Err(ControlTlsError::CertificateIdentityMismatch);
    };
    let certificate = match candidate.confirm_fingerprint(expected.fingerprint) {
        Ok(certificate) => certificate,
        Err(error) => {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(error);
        }
    };
    if socket.set_read_timeout(original_read_timeout).is_err()
        || socket.set_write_timeout(original_write_timeout).is_err()
    {
        let _ = socket.shutdown(Shutdown::Both);
        return Err(ControlTlsError::CertificateExchangeFailed);
    }
    Ok(certificate)
}

/// Exchange and parse a bounded peer certificate for display during explicit
/// enrollment. The returned candidate is untrusted and cannot be used by the
/// TLS factories until its fingerprint is independently confirmed.
pub fn exchange_control_tls_candidate(
    mut socket: TcpStream,
    local: &LocalControlTlsIdentity,
    network: NetworkId,
    deadline: Instant,
) -> Result<ControlTlsCertificateCandidate, ControlTlsError> {
    if is_zero_network(network) {
        let _ = socket.shutdown(Shutdown::Both);
        return Err(ControlTlsError::InvalidIdentity);
    }
    let original_read_timeout = socket
        .read_timeout()
        .map_err(|_| ControlTlsError::CertificateExchangeFailed)?;
    let original_write_timeout = socket
        .write_timeout()
        .map_err(|_| ControlTlsError::CertificateExchangeFailed)?;
    let candidate =
        match exchange_control_tls_candidate_inner(&mut socket, local, network, deadline) {
            Ok(candidate) => candidate,
            Err(error) => {
                let _ = socket.shutdown(Shutdown::Both);
                return Err(error);
            }
        };
    if socket.set_read_timeout(original_read_timeout).is_err()
        || socket.set_write_timeout(original_write_timeout).is_err()
    {
        let _ = socket.shutdown(Shutdown::Both);
        return Err(ControlTlsError::CertificateExchangeFailed);
    }
    Ok(candidate)
}

fn exchange_control_tls_candidate_inner(
    socket: &mut TcpStream,
    local: &LocalControlTlsIdentity,
    network: NetworkId,
    deadline: Instant,
) -> Result<ControlTlsCertificateCandidate, ControlTlsError> {
    let local_certificate = local.certificate.as_ref();
    let length =
        u32::try_from(local_certificate.len()).map_err(|_| ControlTlsError::InvalidCertificate)?;
    let mut preface = Vec::with_capacity(CERTIFICATE_PREFACE.len() + 4 + local_certificate.len());
    preface.extend_from_slice(CERTIFICATE_PREFACE);
    preface.extend_from_slice(&length.to_be_bytes());
    preface.extend_from_slice(local_certificate);
    write_until(socket, &preface, deadline)?;

    let mut header = vec![0_u8; CERTIFICATE_PREFACE.len() + 4];
    read_exact_until(socket, &mut header, deadline)?;
    if !header.starts_with(CERTIFICATE_PREFACE) {
        return Err(ControlTlsError::CertificateExchangeFailed);
    }
    let length_offset = CERTIFICATE_PREFACE.len();
    let certificate_len = u32::from_be_bytes(
        header[length_offset..]
            .try_into()
            .map_err(|_| ControlTlsError::CertificateExchangeFailed)?,
    ) as usize;
    if certificate_len == 0 || certificate_len > MAX_CERTIFICATE_BYTES {
        return Err(ControlTlsError::InvalidCertificate);
    }
    let mut certificate = vec![0_u8; certificate_len];
    read_exact_until(socket, &mut certificate, deadline)?;
    if Instant::now() >= deadline {
        return Err(ControlTlsError::CertificateExchangeDeadlineExpired);
    }

    let certificate = CertificateDer::from(certificate);
    let node = certificate_node_id(&certificate)?;
    if is_zero_node(node) || node == local.node {
        return Err(ControlTlsError::InvalidIdentity);
    }
    let fingerprint = certificate_fingerprint(&certificate)?;
    Ok(ControlTlsCertificateCandidate {
        network,
        node,
        certificate,
        fingerprint,
        observed_at: Instant::now(),
    })
}

fn write_until(
    socket: &mut TcpStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), ControlTlsError> {
    let mut written = 0;
    while written < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ControlTlsError::CertificateExchangeDeadlineExpired);
        }
        socket
            .set_write_timeout(Some(remaining))
            .map_err(|_| ControlTlsError::CertificateExchangeFailed)?;
        match socket.write(&bytes[written..]) {
            Ok(0) => return Err(ControlTlsError::CertificateExchangeFailed),
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(ControlTlsError::CertificateExchangeDeadlineExpired);
            }
            Err(_) => return Err(ControlTlsError::CertificateExchangeFailed),
        }
    }
    if Instant::now() >= deadline {
        return Err(ControlTlsError::CertificateExchangeDeadlineExpired);
    }
    Ok(())
}

fn read_exact_until(
    socket: &mut TcpStream,
    bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), ControlTlsError> {
    let mut received = 0;
    while received < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ControlTlsError::CertificateExchangeDeadlineExpired);
        }
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|_| ControlTlsError::CertificateExchangeFailed)?;
        match socket.read(&mut bytes[received..]) {
            Ok(0) => return Err(ControlTlsError::CertificateExchangeFailed),
            Ok(count) => received += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(ControlTlsError::CertificateExchangeDeadlineExpired);
            }
            Err(_) => return Err(ControlTlsError::CertificateExchangeFailed),
        }
    }
    if Instant::now() >= deadline {
        return Err(ControlTlsError::CertificateExchangeDeadlineExpired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use std::io::Write;

    use std::thread;

    use super::super::test_support::{make_tls_pair, network, stream_pair};
    use super::super::{CERTIFICATE_PREFACE, ControlTlsError, MAX_ENROLLMENT_CANDIDATE_AGE};
    use super::*;
    use crate::control_tls::certificate::PinnedControlTlsPeer;
    use blueroute_protocol::peer_pin::SpkiSha256;
    use std::time::Duration;

    #[test]
    fn certificate_candidate_exchange_rejects_trailing_der_before_enrollment() {
        let (candidate, server) = make_tls_pair();
        let (mut candidate_socket, server_socket) = stream_pair();
        let server_thread = thread::spawn(move || {
            exchange_control_tls_candidate(
                server_socket,
                &server,
                network(9),
                Instant::now() + Duration::from_secs(5),
            )
        });
        // Present a valid node certificate with an extra ASN.1 value in
        // the same bounded certificate preface. A peer-controlled suffix
        // must not be silently ignored by the first-key parser.
        let mut bad_certificate = candidate.certificate().as_ref().to_vec();
        bad_certificate.extend_from_slice(&[0x05, 0x00]);
        let mut preface = Vec::new();
        preface.extend_from_slice(CERTIFICATE_PREFACE);
        preface.extend_from_slice(&(bad_certificate.len() as u32).to_be_bytes());
        preface.extend_from_slice(&bad_certificate);
        candidate_socket.write_all(&preface).unwrap();
        assert!(matches!(
            server_thread.join().unwrap(),
            Err(ControlTlsError::InvalidCertificate)
        ));
    }

    #[test]
    fn expired_enrollment_candidate_cannot_be_revived_by_new_confirmation() {
        let (client, server) = make_tls_pair();
        let expected = server.fingerprint();
        let (client_socket, server_socket) = stream_pair();
        let server_thread = thread::spawn(move || {
            exchange_control_tls_candidate(
                server_socket,
                &server,
                network(9),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        });
        let candidate = exchange_control_tls_candidate(
            client_socket,
            &client,
            network(9),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        server_thread.join().unwrap();

        // Even an exactly matching independently provided key does not
        // authorize a candidate that has outlived its observation window.
        let mut stale = candidate.clone();
        stale.observed_at = Instant::now() - MAX_ENROLLMENT_CANDIDATE_AGE - Duration::from_secs(1);
        assert_eq!(
            stale.confirm_fingerprint(expected).unwrap_err(),
            ControlTlsError::EnrollmentCandidateExpired
        );
        // A cloned candidate cannot gain a new lifetime from a later caller
        // deadline. A genuinely fresh candidate still permits confirmation.
        assert!(candidate.confirm_fingerprint(expected).is_ok());
    }

    #[test]
    fn certificate_preface_rejects_a_valid_but_unapproved_key() {
        let (client, server) = make_tls_pair();
        let client_allowed =
            PinnedControlTlsPeer::new(network(9), client.node(), client.fingerprint()).unwrap();
        let wrong_server_fingerprint =
            SpkiSha256::from_verified_certificate_digest([99; 32]).unwrap();
        let server_allowed =
            PinnedControlTlsPeer::new(network(9), server.node(), wrong_server_fingerprint).unwrap();
        let (client_socket, server_socket) = stream_pair();
        let server_thread = thread::spawn(move || {
            let mut socket = server_socket;
            exchange_control_tls_certificate(
                &mut socket,
                &server,
                network(9),
                &[client_allowed],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        });
        let mut socket = client_socket;
        assert_eq!(
            exchange_control_tls_certificate(
                &mut socket,
                &client,
                network(9),
                &[server_allowed],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap_err(),
            ControlTlsError::FingerprintMismatch
        );
        server_thread.join().unwrap();
    }
}

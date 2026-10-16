//! Pinned TLS 1.3 identities for BlueRoute's inter-node control channel.
//!
//! TLS uses standard X.509 certificates and the rustls ring provider. The
//! certificate's SubjectPublicKeyInfo is pinned to a separately approved
//! `(NetworkId, NodeId)` by the peer-pin policy. A TLS handshake must still
//! prove possession of the matching private key. This module does not enroll
//! peers or activate JoinNetwork.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId, Secret};
use blueroute_linux::SecretFileStore;
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::control_io::{DeadlineControlRead, DeadlineControlWrite};
use blueroute_protocol::handshake::tls_control_exporter_context;
use blueroute_protocol::handshake::{HelloExchangeRequest, exchange_hello_claims_with_deadline};
use blueroute_protocol::handshake::{TLS_CONTROL_EXPORTER_BYTES, TLS_CONTROL_EXPORTER_LABEL};
use blueroute_protocol::peer_pin::SpkiSha256;
use blueroute_protocol::session::ConnectionDeadline;
use blueroute_protocol::session_writer::SessionControlWriter;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig, SupportedProtocolVersion};
use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, X509Certificate};
use zeroize::Zeroize;

static TLS13_ONLY: [&SupportedProtocolVersion; 1] = [&rustls::version::TLS13];
const MAX_CERTIFICATE_BYTES: usize = 16 * 1024;
const MAX_PRIVATE_KEY_BYTES: usize = 16 * 1024;
const CERTIFICATE_PREFACE: &[u8] = b"BLUEROUTE-CONTROL-CERT-V1\0";
const IDENTITY_FILE: &str = "control-tls-identity-v1";
const IDENTITY_HEADER: &[u8] = b"BLUEROUTE_CONTROL_TLS_IDENTITY_V1\0";

/// A local X.509 TLS identity. Callers should keep this value in daemon
/// lifetime memory and persist its private DER only through `SecretFileStore`.
pub struct LocalControlTlsIdentity {
    node: NodeId,
    certificate: CertificateDer<'static>,
    private_key: PrivateKeyDer<'static>,
    fingerprint: SpkiSha256,
}

impl fmt::Debug for LocalControlTlsIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalControlTlsIdentity")
            .field("node", &self.node)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl Drop for LocalControlTlsIdentity {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

impl LocalControlTlsIdentity {
    /// Generate a new local certificate and matching private key.
    ///
    /// The returned private key must be persisted before it is used by a
    /// service. This convenience constructor is primarily useful for first
    /// enrollment and deterministic integration tests.
    pub fn generate(node: NodeId) -> Result<Self, ControlTlsError> {
        if is_zero_node(node) {
            return Err(ControlTlsError::InvalidIdentity);
        }
        let key_pair = KeyPair::generate().map_err(|_| ControlTlsError::InvalidKey)?;
        let certificate = CertificateParams::new(vec![node_dns_name(node)])
            .map_err(|_| ControlTlsError::InvalidCertificate)?
            .self_signed(&key_pair)
            .map_err(|_| ControlTlsError::InvalidCertificate)?;
        Self::from_der(
            node,
            certificate.der().clone(),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der())),
        )
    }

    /// Load a locally persisted identity and check that the certificate
    /// contains this node's DNS identity and matches its private key.
    pub fn from_der(
        node: NodeId,
        certificate: CertificateDer<'static>,
        private_key: PrivateKeyDer<'static>,
    ) -> Result<Self, ControlTlsError> {
        if is_zero_node(node) {
            return Err(ControlTlsError::InvalidIdentity);
        }
        if certificate.len() > MAX_CERTIFICATE_BYTES {
            return Err(ControlTlsError::InvalidCertificate);
        }
        let fingerprint = certificate_fingerprint(&certificate)?;
        if !certificate_names_node(&certificate, node)? {
            return Err(ControlTlsError::CertificateIdentityMismatch);
        }

        let provider = rustls::crypto::ring::default_provider();
        rustls::sign::CertifiedKey::from_der(
            vec![certificate.clone()],
            private_key.clone_key(),
            &provider,
        )
        .map_err(|_| ControlTlsError::KeyCertificateMismatch)?
        .keys_match()
        .map_err(|_| ControlTlsError::KeyCertificateMismatch)?;

        Ok(Self {
            node,
            certificate,
            private_key,
            fingerprint,
        })
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }

    pub fn certificate(&self) -> &CertificateDer<'static> {
        &self.certificate
    }

    /// Build a client configuration that trusts exactly one already-enrolled
    /// peer certificate and always presents this node's own certificate.
    pub fn client_config(
        &self,
        network: NetworkId,
        peer: &PinnedControlTlsCertificate,
    ) -> Result<(Arc<ClientConfig>, ServerName<'static>), ControlTlsError> {
        if is_zero_network(network) || peer.network != network || peer.node == self.node {
            return Err(ControlTlsError::InvalidIdentity);
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(peer.certificate.clone())
            .map_err(|_| ControlTlsError::InvalidCertificate)?;

        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&TLS13_ONLY)
                .map_err(|_| ControlTlsError::InvalidConfiguration)?
                .with_root_certificates(roots)
                .with_client_auth_cert(vec![self.certificate.clone()], self.private_key.clone_key())
                .map_err(|_| ControlTlsError::InvalidConfiguration)?;

        let name = ServerName::try_from(node_dns_name(peer.node))
            .map_err(|_| ControlTlsError::InvalidIdentity)?;
        Ok((Arc::new(config), name))
    }

    /// Build a server configuration requiring a certificate from every peer
    /// in the current approved-and-pinned set. The caller must reload that set
    /// after trust changes and close sessions when a peer is revoked.
    pub fn server_config(
        &self,
        network: NetworkId,
        peers: &[PinnedControlTlsCertificate],
    ) -> Result<Arc<ServerConfig>, ControlTlsError> {
        if is_zero_network(network)
            || peers.is_empty()
            || peers
                .iter()
                .any(|peer| peer.network != network || peer.node == self.node)
        {
            return Err(ControlTlsError::InvalidIdentity);
        }
        let mut roots = RootCertStore::empty();
        for peer in peers {
            roots
                .add(peer.certificate.clone())
                .map_err(|_| ControlTlsError::InvalidCertificate)?;
        }
        let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|_| ControlTlsError::InvalidConfiguration)?;

        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&TLS13_ONLY)
                .map_err(|_| ControlTlsError::InvalidConfiguration)?
                .with_client_cert_verifier(verifier)
                .with_single_cert(vec![self.certificate.clone()], self.private_key.clone_key())
                .map_err(|_| ControlTlsError::InvalidConfiguration)?;
        Ok(Arc::new(config))
    }
}

/// Owner-only persistent local TLS identity storage.
pub struct ControlTlsIdentityStore {
    secrets: SecretFileStore,
}

impl ControlTlsIdentityStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            secrets: SecretFileStore::new(directory),
        }
    }

    /// Load the existing identity or atomically create it once. A partial,
    /// malformed, mismatched, or wrong-node record fails closed and is never
    /// replaced with a new key.
    pub fn load_or_create(&self, node: NodeId) -> Result<LocalControlTlsIdentity, CoreError> {
        if let Some(mut record) = self.secrets.load(IDENTITY_FILE)? {
            let identity = decode_identity_record(record.expose(), node);
            record.expose_mut().zeroize();
            return identity;
        }

        let generated = LocalControlTlsIdentity::generate(node).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to generate the local TLS identity",
                error.to_string(),
            )
        })?;
        let bytes = encode_identity_record(&generated)?;
        let mut secret = Secret::new(bytes);
        let created = self.secrets.save_if_absent(IDENTITY_FILE, &secret);
        secret.expose_mut().zeroize();
        if created? {
            return Ok(generated);
        }

        let mut record = self.secrets.load(IDENTITY_FILE)?.ok_or_else(|| {
            CoreError::new(
                ErrorKind::PersistenceError,
                "local TLS identity disappeared during first-start creation",
            )
        })?;
        let identity = decode_identity_record(record.expose(), node);
        record.expose_mut().zeroize();
        identity
    }
}

fn encode_identity_record(identity: &LocalControlTlsIdentity) -> Result<Vec<u8>, CoreError> {
    let certificate = identity.certificate.as_ref();
    let private_key = identity.private_key.secret_der();
    if certificate.len() > MAX_CERTIFICATE_BYTES || private_key.len() > MAX_PRIVATE_KEY_BYTES {
        return Err(invalid_identity_record(
            "local TLS identity exceeds size limits",
        ));
    }
    let certificate_len = u32::try_from(certificate.len())
        .map_err(|_| invalid_identity_record("local TLS certificate length is invalid"))?;
    let key_len = u32::try_from(private_key.len())
        .map_err(|_| invalid_identity_record("local TLS private-key length is invalid"))?;
    let mut record =
        Vec::with_capacity(IDENTITY_HEADER.len() + 8 + certificate.len() + private_key.len());
    record.extend_from_slice(IDENTITY_HEADER);
    record.extend_from_slice(&certificate_len.to_be_bytes());
    record.extend_from_slice(&key_len.to_be_bytes());
    record.extend_from_slice(certificate);
    record.extend_from_slice(private_key);
    Ok(record)
}

fn decode_identity_record(
    record: &[u8],
    node: NodeId,
) -> Result<LocalControlTlsIdentity, CoreError> {
    let minimum = IDENTITY_HEADER.len() + 8;
    if record.len() < minimum || !record.starts_with(IDENTITY_HEADER) {
        return Err(invalid_identity_record(
            "local TLS identity record is malformed",
        ));
    }
    let certificate_len = u32::from_be_bytes(
        record[IDENTITY_HEADER.len()..IDENTITY_HEADER.len() + 4]
            .try_into()
            .map_err(|_| invalid_identity_record("local TLS identity record is malformed"))?,
    ) as usize;
    let key_len = u32::from_be_bytes(
        record[IDENTITY_HEADER.len() + 4..minimum]
            .try_into()
            .map_err(|_| invalid_identity_record("local TLS identity record is malformed"))?,
    ) as usize;
    if certificate_len == 0
        || certificate_len > MAX_CERTIFICATE_BYTES
        || key_len == 0
        || key_len > MAX_PRIVATE_KEY_BYTES
        || minimum + certificate_len + key_len != record.len()
    {
        return Err(invalid_identity_record(
            "local TLS identity record is malformed",
        ));
    }
    let certificate = CertificateDer::from(record[minimum..minimum + certificate_len].to_vec());
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        record[minimum + certificate_len..].to_vec(),
    ));
    LocalControlTlsIdentity::from_der(node, certificate, private_key).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::PersistenceError,
            "persisted local TLS identity is invalid",
            error.to_string(),
        )
    })
}

fn invalid_identity_record(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::PersistenceError, message)
}

/// A peer certificate whose NodeId and SPKI matched an expected fingerprint.
/// Durable membership and pin authorization are still rechecked before a
/// control session is activated.
#[derive(Clone, Debug)]
pub struct PinnedControlTlsCertificate {
    network: NetworkId,
    node: NodeId,
    certificate: CertificateDer<'static>,
    fingerprint: SpkiSha256,
}

/// Untrusted certificate identity received during the enrollment ceremony.
/// It must not authorize a control session unless a user independently
/// confirms the exact fingerprint and `confirm_fingerprint` succeeds.
#[derive(Clone, Debug)]
pub struct ControlTlsCertificateCandidate {
    network: NetworkId,
    node: NodeId,
    certificate: CertificateDer<'static>,
    fingerprint: SpkiSha256,
}

impl ControlTlsCertificateCandidate {
    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }

    /// Convert candidate material into an exact-leaf TLS trust input only
    /// when the independently confirmed fingerprint matches it.
    pub fn confirm_fingerprint(
        self,
        independently_confirmed: SpkiSha256,
    ) -> Result<PinnedControlTlsCertificate, ControlTlsError> {
        if independently_confirmed != self.fingerprint {
            return Err(ControlTlsError::FingerprintMismatch);
        }
        PinnedControlTlsCertificate::new(
            self.network,
            self.node,
            self.certificate,
            independently_confirmed,
        )
    }
}

impl PinnedControlTlsCertificate {
    pub fn new(
        network: NetworkId,
        node: NodeId,
        certificate: CertificateDer<'static>,
        expected_fingerprint: SpkiSha256,
    ) -> Result<Self, ControlTlsError> {
        if is_zero_network(network) || is_zero_node(node) {
            return Err(ControlTlsError::InvalidIdentity);
        }
        if certificate.len() > MAX_CERTIFICATE_BYTES {
            return Err(ControlTlsError::InvalidCertificate);
        }
        let fingerprint = certificate_fingerprint(&certificate)?;
        if fingerprint != expected_fingerprint {
            return Err(ControlTlsError::FingerprintMismatch);
        }
        if !certificate_names_node(&certificate, node)? {
            return Err(ControlTlsError::CertificateIdentityMismatch);
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(certificate.clone())
            .map_err(|_| ControlTlsError::InvalidCertificate)?;
        Ok(Self {
            network,
            node,
            certificate,
            fingerprint,
        })
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }

    pub const fn peer_pin(&self) -> PinnedControlTlsPeer {
        PinnedControlTlsPeer {
            network: self.network,
            node: self.node,
            fingerprint: self.fingerprint,
        }
    }
}

/// Independently approved peer identity used to validate the bounded cleartext
/// certificate preface before configuring TLS. The preface carries no trusted
/// claims: this pin must match it, then TLS must prove private-key possession.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinnedControlTlsPeer {
    network: NetworkId,
    node: NodeId,
    fingerprint: SpkiSha256,
}

impl PinnedControlTlsPeer {
    pub fn new(
        network: NetworkId,
        node: NodeId,
        fingerprint: SpkiSha256,
    ) -> Result<Self, ControlTlsError> {
        if is_zero_network(network) || is_zero_node(node) {
            return Err(ControlTlsError::InvalidIdentity);
        }
        Ok(Self {
            network,
            node,
            fingerprint,
        })
    }

    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }
}

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
    if is_zero_network(network)
        || allowed_peers.is_empty()
        || allowed_peers.iter().any(|peer| {
            peer.network != network
                || peer.node == local.node
                || allowed_peers
                    .iter()
                    .filter(|other| other.node == peer.node)
                    .count()
                    != 1
                || allowed_peers
                    .iter()
                    .filter(|other| other.fingerprint == peer.fingerprint)
                    .count()
                    != 1
        })
    {
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

/// Exchange and pin-check the peer certificate, then establish the client side
/// of a TLS 1.3 mutual-authenticated connection by one absolute deadline.
pub fn connect_pinned_control_tls(
    mut socket: TcpStream,
    local: &LocalControlTlsIdentity,
    network: NetworkId,
    expected_peer: PinnedControlTlsPeer,
    deadline: Instant,
) -> Result<(ControlTlsStream, PinnedControlTlsCertificate), ControlTlsError> {
    let peer_certificate =
        exchange_control_tls_certificate(&mut socket, local, network, &[expected_peer], deadline)?;
    let (config, server_name) = match local.client_config(network, &peer_certificate) {
        Ok(config) => config,
        Err(error) => {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(error);
        }
    };
    let connection = match rustls::ClientConnection::new(config, server_name) {
        Ok(connection) => connection,
        Err(_) => {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(ControlTlsError::InvalidConfiguration);
        }
    };
    let mut stream = ControlTlsStream::client(connection, socket);
    stream.complete_handshake_until(deadline)?;
    Ok((stream, peer_certificate))
}

/// Match an inbound certificate preface against the current approved pin set,
/// then establish the server side of a TLS 1.3 mutual-authenticated connection.
pub fn accept_pinned_control_tls(
    mut socket: TcpStream,
    local: &LocalControlTlsIdentity,
    network: NetworkId,
    allowed_peers: &[PinnedControlTlsPeer],
    deadline: Instant,
) -> Result<(ControlTlsStream, PinnedControlTlsCertificate), ControlTlsError> {
    let peer_certificate =
        exchange_control_tls_certificate(&mut socket, local, network, allowed_peers, deadline)?;
    let config = match local.server_config(network, std::slice::from_ref(&peer_certificate)) {
        Ok(config) => config,
        Err(error) => {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(error);
        }
    };
    let connection = match rustls::ServerConnection::new(config) {
        Ok(connection) => connection,
        Err(_) => {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(ControlTlsError::InvalidConfiguration);
        }
    };
    let mut stream = ControlTlsStream::server(connection, socket);
    stream.complete_handshake_until(deadline)?;
    Ok((stream, peer_certificate))
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

/// Identity authenticated by the completed TLS handshake and matched to one
/// current enrolled certificate. Its fields cannot be supplied by callers.
#[derive(Clone, Debug)]
pub struct VerifiedControlTlsPeer {
    network: NetworkId,
    node: NodeId,
    fingerprint: SpkiSha256,
}

/// A TLS-protected TCP stream. The TLS connection and its socket stay paired
/// so exporter material and Hello frames necessarily use the same channel.
pub struct ControlTlsStream {
    connection: rustls::Connection,
    socket: TcpStream,
}

impl fmt::Debug for ControlTlsStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlTlsStream")
            .field("handshaking", &self.connection.is_handshaking())
            .finish_non_exhaustive()
    }
}

impl ControlTlsStream {
    pub fn client(connection: rustls::ClientConnection, socket: TcpStream) -> Self {
        Self {
            connection: rustls::Connection::Client(connection),
            socket,
        }
    }

    pub fn server(connection: rustls::ServerConnection, socket: TcpStream) -> Self {
        Self {
            connection: rustls::Connection::Server(connection),
            socket,
        }
    }

    /// Complete TLS within one absolute deadline. Any failure shuts down the
    /// socket; callers must discard this transport and start a new attempt.
    pub fn complete_handshake_until(&mut self, deadline: Instant) -> Result<(), ControlTlsError> {
        if Instant::now() >= deadline {
            self.close();
            return Err(ControlTlsError::HandshakeDeadlineExpired);
        }
        let original_read_timeout = self
            .socket
            .read_timeout()
            .map_err(|_| ControlTlsError::HandshakeFailed)?;
        let original_write_timeout = self
            .socket
            .write_timeout()
            .map_err(|_| ControlTlsError::HandshakeFailed)?;
        while self.connection.is_handshaking() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.close();
                return Err(ControlTlsError::HandshakeDeadlineExpired);
            }
            if self.socket.set_read_timeout(Some(remaining)).is_err()
                || self.socket.set_write_timeout(Some(remaining)).is_err()
            {
                self.close();
                return Err(ControlTlsError::HandshakeFailed);
            }
            if let Err(error) = self.connection.complete_io(&mut self.socket) {
                let expired = Instant::now() >= deadline
                    || matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    );
                self.close();
                return Err(if expired {
                    ControlTlsError::HandshakeDeadlineExpired
                } else {
                    ControlTlsError::HandshakeFailed
                });
            }
            if Instant::now() >= deadline {
                self.close();
                return Err(ControlTlsError::HandshakeDeadlineExpired);
            }
        }
        if self.socket.set_read_timeout(original_read_timeout).is_err()
            || self
                .socket
                .set_write_timeout(original_write_timeout)
                .is_err()
        {
            self.close();
            return Err(ControlTlsError::HandshakeFailed);
        }
        Ok(())
    }

    /// Verify the peer certificate, bind a fresh exporter to its identity,
    /// then exchange the symmetric Hello/HelloAck on this same TLS stream.
    /// `expected` must come from the durable approved key-pin registry.
    pub fn exchange_authenticated_hello(
        &mut self,
        network: NetworkId,
        local: &LocalControlTlsIdentity,
        expected: &PinnedControlTlsCertificate,
        local_facts: HelloFacts,
        deadline: Instant,
    ) -> Result<AuthenticatedControlPeer, ControlTlsError> {
        let peer = match verify_control_tls_peer(&self.connection, network, expected) {
            Ok(peer) => peer,
            Err(error) => {
                self.close();
                return Err(error);
            }
        };
        let session_id =
            match export_control_session_id(&mut self.connection, network, local, &peer) {
                Ok(session_id) => session_id,
                Err(error) => {
                    self.close();
                    return Err(error);
                }
            };
        let mut local_challenge = [0_u8; 16];
        if getrandom::fill(&mut local_challenge).is_err() {
            self.close();
            return Err(ControlTlsError::RandomnessUnavailable);
        }
        if local_challenge.iter().all(|byte| *byte == 0) {
            self.close();
            return Err(ControlTlsError::RandomnessUnavailable);
        }
        let facts = exchange_hello_claims_with_deadline(
            self,
            HelloExchangeRequest {
                network,
                local: local.node(),
                remote: peer.node(),
                exported_session_id: session_id,
                local_challenge,
                local_facts,
                deadline,
            },
        )
        .map_err(|_| {
            self.close();
            ControlTlsError::HelloExchangeFailed
        })?;
        Ok(AuthenticatedControlPeer {
            peer,
            session_id,
            facts,
        })
    }

    /// Bind the Hello result back to this exact TLS connection before opening
    /// durable pin-gated ingress. An authenticated result from another stream
    /// cannot be substituted because its exporter will differ.
    pub fn activate<'a>(
        mut self,
        authenticated: AuthenticatedControlPeer,
        local: &LocalControlTlsIdentity,
        trust: &'a crate::DurablePeerTrustOperations,
        deadline: ConnectionDeadline,
    ) -> Result<ActiveControlSession<'a>, ControlSessionError> {
        let network = authenticated.peer.network();
        let peer = match verify_control_tls_peer_identity(
            &self.connection,
            network,
            authenticated.peer.node(),
            authenticated.peer.fingerprint(),
        ) {
            Ok(peer) => peer,
            Err(error) => {
                self.close();
                return Err(ControlSessionError::Authentication(error));
            }
        };
        let session_id =
            match export_control_session_id(&mut self.connection, network, local, &peer) {
                Ok(session_id) if session_id == authenticated.session_id => session_id,
                Ok(_) => {
                    self.close();
                    return Err(ControlSessionError::Authentication(
                        ControlTlsError::ExporterMismatch,
                    ));
                }
                Err(error) => {
                    self.close();
                    return Err(ControlSessionError::Authentication(error));
                }
            };
        let identity = crate::VerifiedTlsIdentity::from_control_tls(
            network,
            peer.node(),
            peer.fingerprint(),
            session_id,
        );
        let ingress = match crate::PinnedSessionIngress::after_verified_handshake(
            trust, identity, deadline,
        ) {
            Ok(ingress) => ingress,
            Err(error) => {
                self.close();
                return Err(ControlSessionError::Ingress(error));
            }
        };
        let writer = SessionControlWriter::after_verified_hello(
            network,
            local.node(),
            peer.node(),
            session_id,
        )
        .map_err(|_| ControlSessionError::Authentication(ControlTlsError::InvalidIdentity))?;
        Ok(ActiveControlSession {
            stream: self,
            ingress,
            trust,
            writer,
            peer,
            session_id,
            facts: authenticated.facts,
            terminal: false,
        })
    }

    pub fn close(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    fn send_close_notify(&mut self) -> io::Result<()> {
        match &mut self.connection {
            rustls::Connection::Client(connection) => connection.send_close_notify(),
            rustls::Connection::Server(connection) => connection.send_close_notify(),
        }
        self.flush()?;
        self.socket.shutdown(Shutdown::Write)
    }
}

impl Read for ControlTlsStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.connection {
            rustls::Connection::Client(connection) => {
                rustls::Stream::new(connection, &mut self.socket).read(buffer)
            }
            rustls::Connection::Server(connection) => {
                rustls::Stream::new(connection, &mut self.socket).read(buffer)
            }
        }
    }
}

impl Write for ControlTlsStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match &mut self.connection {
            rustls::Connection::Client(connection) => {
                rustls::Stream::new(connection, &mut self.socket).write(buffer)
            }
            rustls::Connection::Server(connection) => {
                rustls::Stream::new(connection, &mut self.socket).write(buffer)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.connection {
            rustls::Connection::Client(connection) => {
                rustls::Stream::new(connection, &mut self.socket).flush()
            }
            rustls::Connection::Server(connection) => {
                rustls::Stream::new(connection, &mut self.socket).flush()
            }
        }
    }
}

impl DeadlineControlRead for ControlTlsStream {
    fn set_control_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.socket.set_read_timeout(Some(timeout))
    }
}

impl DeadlineControlWrite for ControlTlsStream {
    fn set_control_write_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.socket.set_write_timeout(Some(timeout))
    }
}

/// Peer identity and capability claims returned only after mutual TLS and the
/// exporter-bound Hello/HelloAck transcript both succeed.
#[derive(Debug)]
pub struct AuthenticatedControlPeer {
    peer: VerifiedControlTlsPeer,
    session_id: [u8; TLS_CONTROL_EXPORTER_BYTES],
    facts: HelloFacts,
}

impl AuthenticatedControlPeer {
    pub const fn peer(&self) -> &VerifiedControlTlsPeer {
        &self.peer
    }

    pub const fn session_id(&self) -> &[u8; TLS_CONTROL_EXPORTER_BYTES] {
        &self.session_id
    }

    pub const fn facts(&self) -> &HelloFacts {
        &self.facts
    }
}

/// A channel-bound, durably authorized active control session. The TLS stream
/// and ingress parser are owned together and cannot be substituted separately.
pub struct ActiveControlSession<'a> {
    stream: ControlTlsStream,
    ingress: crate::PinnedSessionIngress<'a>,
    trust: &'a crate::DurablePeerTrustOperations,
    writer: SessionControlWriter,
    peer: VerifiedControlTlsPeer,
    session_id: [u8; TLS_CONTROL_EXPORTER_BYTES],
    facts: HelloFacts,
    terminal: bool,
}

impl ActiveControlSession<'_> {
    pub const fn peer(&self) -> &VerifiedControlTlsPeer {
        &self.peer
    }

    pub const fn session_id(&self) -> &[u8; TLS_CONTROL_EXPORTER_BYTES] {
        &self.session_id
    }

    pub const fn facts(&self) -> &HelloFacts {
        &self.facts
    }

    pub const fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Force-close the owned TLS socket before releasing the caller's session
    /// registry slot, so a new peer generation cannot overlap teardown.
    pub(crate) fn close_owned_socket(&mut self) {
        self.terminal = true;
        self.stream.close();
    }

    pub fn send_heartbeat_until(&mut self, deadline: Instant) -> Result<(), ControlSessionError> {
        self.authorize_outbound()?;
        if self
            .writer
            .write_heartbeat_with_deadline(&mut self.stream, deadline)
            .is_err()
        {
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::OutboundFailure);
        }
        Ok(())
    }

    /// Send the terminal Goodbye and TLS close-notify. The peer's clean EOF
    /// still needs to be consumed by `receive_next_at` before this closes.
    pub fn send_goodbye_until(&mut self, deadline: Instant) -> Result<(), ControlSessionError> {
        self.authorize_outbound()?;
        if self
            .writer
            .write_goodbye_with_deadline(&mut self.stream, deadline)
            .is_err()
            || self.stream.send_close_notify().is_err()
        {
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::OutboundFailure);
        }
        Ok(())
    }

    fn authorize_outbound(&mut self) -> Result<(), ControlSessionError> {
        if self.terminal {
            return Err(ControlSessionError::Ingress(
                crate::PinnedIngressError::Terminal,
            ));
        }
        self.trust
            .require_approved_pinned_tls_peer(
                self.peer.network(),
                self.peer.node(),
                self.peer.fingerprint(),
            )
            .map_err(|error| {
                self.terminal = true;
                self.stream.close();
                ControlSessionError::Ingress(crate::PinnedIngressError::Authorization(error))
            })
    }

    /// Read one TLS plaintext chunk under the session's current idle deadline,
    /// validate it against durable peer approval, and terminally close on any
    /// framing, authorization or transport failure.
    pub fn receive_next_at(
        &mut self,
        now_ms: u64,
    ) -> Result<ControlSessionRead, ControlSessionError> {
        if self.terminal {
            return Err(ControlSessionError::Ingress(
                crate::PinnedIngressError::Terminal,
            ));
        }
        let expires_ms = self.ingress.expires_ms();
        if now_ms >= expires_ms {
            let result = self.ingress.receive_verified_plaintext(&[], now_ms);
            self.terminal = true;
            self.stream.close();
            return match result {
                Err(error) => Err(ControlSessionError::Ingress(error)),
                Ok(_) => Err(ControlSessionError::IdleDeadlineExpired),
            };
        }

        // Do not enter a blocking read with a peer whose durable approval has
        // been revoked, or with a regressed session clock.
        if let Err(error) = self.ingress.check_before_read(now_ms) {
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::Ingress(error));
        }

        let timeout = Duration::from_millis(expires_ms - now_ms);
        if let Err(error) = self.stream.socket.set_read_timeout(Some(timeout)) {
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::Transport(error));
        }
        let mut plaintext = [0_u8; 4096];
        let read_started = Instant::now();
        let read_deadline = read_started + timeout;
        let read_result = self.stream.read(&mut plaintext);
        let read_finished = Instant::now();
        // The completion time, not the caller's pre-read time, determines
        // whether a late-arriving frame may refresh the idle deadline.
        if read_finished >= read_deadline {
            let _ = self.ingress.receive_verified_plaintext(&[], expires_ms);
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::IdleDeadlineExpired);
        }
        let elapsed_ms = read_finished.duration_since(read_started).as_millis();
        let elapsed_ms = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
        let observed_now_ms = now_ms.saturating_add(elapsed_ms);
        match read_result {
            Ok(0) => {
                self.terminal = true;
                self.stream.close();
                self.ingress
                    .finish_at(observed_now_ms)
                    .map(|()| ControlSessionRead::Closed)
                    .map_err(ControlSessionError::Ingress)
            }
            Ok(count) => match self
                .ingress
                .receive_verified_plaintext(&plaintext[..count], observed_now_ms)
            {
                Ok(frames) => Ok(ControlSessionRead::Frames(frames)),
                Err(error) => {
                    self.terminal = true;
                    self.stream.close();
                    Err(ControlSessionError::Ingress(error))
                }
            },
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                match self
                    .ingress
                    .receive_verified_plaintext(&[], observed_now_ms)
                {
                    Ok(_) => Ok(ControlSessionRead::Idle),
                    Err(error) => {
                        self.terminal = true;
                        self.stream.close();
                        Err(ControlSessionError::Ingress(error))
                    }
                }
            }
            Err(error) => {
                self.terminal = true;
                self.stream.close();
                let _ = self.ingress.finish_at(observed_now_ms);
                Err(ControlSessionError::Transport(error))
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ControlSessionRead {
    Frames(Vec<blueroute_protocol::control::ControlEnvelope>),
    Idle,
    Closed,
}

#[derive(Debug)]
pub enum ControlSessionError {
    Authentication(ControlTlsError),
    Ingress(crate::PinnedIngressError),
    Transport(io::Error),
    IdleDeadlineExpired,
    OutboundFailure,
}

impl fmt::Display for ControlSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authentication(error) => {
                write!(f, "control session authentication failed: {error}")
            }
            Self::Ingress(_) => f.write_str("control session durable authorization failed"),
            Self::Transport(error) => write!(f, "control session transport failed: {error}"),
            Self::IdleDeadlineExpired => f.write_str("control session idle deadline expired"),
            Self::OutboundFailure => f.write_str("control session outbound write failed"),
        }
    }
}

impl std::error::Error for ControlSessionError {}

impl VerifiedControlTlsPeer {
    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }
}

/// Match the certificate actually presented on a completed TLS 1.3
/// connection to the enrolled key and logical peer identity.
pub fn verify_control_tls_peer(
    connection: &rustls::Connection,
    network: NetworkId,
    expected: &PinnedControlTlsCertificate,
) -> Result<VerifiedControlTlsPeer, ControlTlsError> {
    if is_zero_network(network) || expected.network != network {
        return Err(ControlTlsError::InvalidIdentity);
    }
    verify_control_tls_peer_identity(connection, network, expected.node, expected.fingerprint)
}

fn verify_control_tls_peer_identity(
    connection: &rustls::Connection,
    network: NetworkId,
    expected_node: NodeId,
    expected_fingerprint: SpkiSha256,
) -> Result<VerifiedControlTlsPeer, ControlTlsError> {
    if is_zero_network(network) || is_zero_node(expected_node) {
        return Err(ControlTlsError::InvalidIdentity);
    }
    if connection.is_handshaking() {
        return Err(ControlTlsError::HandshakeNotComplete);
    }
    if connection.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
        return Err(ControlTlsError::InvalidConfiguration);
    }
    let certificates = connection
        .peer_certificates()
        .ok_or(ControlTlsError::MissingPeerCertificate)?;
    if certificates.len() != 1 {
        return Err(ControlTlsError::InvalidCertificate);
    }
    let presented = &certificates[0];
    if certificate_fingerprint(presented)? != expected_fingerprint
        || !certificate_names_node(presented, expected_node)?
    {
        return Err(ControlTlsError::FingerprintMismatch);
    }
    Ok(VerifiedControlTlsPeer {
        network,
        node: expected_node,
        fingerprint: expected_fingerprint,
    })
}

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

fn certificate_fingerprint(
    certificate: &CertificateDer<'_>,
) -> Result<SpkiSha256, ControlTlsError> {
    let digest = Sha256::digest(certificate_subject_public_key_info(certificate)?);
    let digest: [u8; 32] = digest.into();
    SpkiSha256::from_verified_certificate_digest(digest).map_err(|_| ControlTlsError::InvalidKey)
}

fn certificate_subject_public_key_info(
    certificate: &CertificateDer<'_>,
) -> Result<Vec<u8>, ControlTlsError> {
    let (_, parsed) = X509Certificate::from_der(certificate.as_ref())
        .map_err(|_| ControlTlsError::InvalidCertificate)?;
    Ok(parsed.public_key().raw.to_vec())
}

fn certificate_names_node(
    certificate: &CertificateDer<'_>,
    node: NodeId,
) -> Result<bool, ControlTlsError> {
    Ok(certificate_node_id(certificate)? == node)
}

fn certificate_node_id(certificate: &CertificateDer<'_>) -> Result<NodeId, ControlTlsError> {
    let (_, parsed) = X509Certificate::from_der(certificate.as_ref())
        .map_err(|_| ControlTlsError::InvalidCertificate)?;
    let Some(names) = parsed
        .subject_alternative_name()
        .map_err(|_| ControlTlsError::InvalidCertificate)?
    else {
        return Err(ControlTlsError::CertificateIdentityMismatch);
    };
    if names.value.general_names.len() != 1 {
        return Err(ControlTlsError::CertificateIdentityMismatch);
    }
    let x509_parser::extensions::GeneralName::DNSName(name) = &names.value.general_names[0] else {
        return Err(ControlTlsError::CertificateIdentityMismatch);
    };
    let value = name
        .strip_prefix("node-")
        .ok_or(ControlTlsError::CertificateIdentityMismatch)?;
    value
        .parse()
        .map_err(|_| ControlTlsError::CertificateIdentityMismatch)
}

fn node_dns_name(node: NodeId) -> String {
    format!("node-{node}")
}

/// Derive the existing protocol's session identifier directly from a
/// completed rustls connection. Both peers get the same value for the same
/// channel; network and node context are canonical and direction independent.
pub fn export_control_session_id(
    connection: &mut rustls::Connection,
    network: NetworkId,
    local: &LocalControlTlsIdentity,
    peer: &VerifiedControlTlsPeer,
) -> Result<[u8; TLS_CONTROL_EXPORTER_BYTES], ControlTlsError> {
    if connection.is_handshaking() {
        return Err(ControlTlsError::HandshakeNotComplete);
    }
    if connection.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
        return Err(ControlTlsError::InvalidConfiguration);
    }
    if is_zero_network(network) || peer.network != network || peer.node == local.node {
        return Err(ControlTlsError::InvalidIdentity);
    }
    verify_control_tls_peer_identity(connection, network, peer.node, peer.fingerprint)?;
    let context = tls_control_exporter_context(network, local.node, peer.node)
        .map_err(|_| ControlTlsError::InvalidIdentity)?;
    connection
        .export_keying_material(
            [0_u8; TLS_CONTROL_EXPORTER_BYTES],
            TLS_CONTROL_EXPORTER_LABEL,
            Some(&context),
        )
        .map_err(|_| ControlTlsError::ExporterUnavailable)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use blueroute_core::{DisplayName, MembershipRegistry, MembershipState, NetworkMembership};
    use blueroute_linux::NetworkMembershipStore;
    use blueroute_protocol::control::ControlMessage;

    use super::*;
    use crate::{DurablePeerTrustOperations, PeerPinStore, PeerTrustOperations};

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
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

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn make_tls_pair() -> (LocalControlTlsIdentity, LocalControlTlsIdentity) {
        let client = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
        (client, server)
    }

    fn stream_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    fn hello_facts(version: &str) -> HelloFacts {
        HelloFacts {
            software_version: version.to_owned(),
            capabilities: blueroute_core::NodeCapabilities::default(),
            segment: None,
            direct_neighbors: Vec::new(),
        }
    }

    fn trust_for_peer(
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

    #[test]
    fn certificate_fingerprint_is_der_spki_and_node_scoped() {
        let identity = LocalControlTlsIdentity::generate(node(7)).unwrap();
        let pinned = PinnedControlTlsCertificate::new(
            network(9),
            node(7),
            identity.certificate().clone(),
            identity.fingerprint(),
        )
        .unwrap();
        assert_eq!(pinned.fingerprint(), identity.fingerprint());
        assert!(matches!(
            PinnedControlTlsCertificate::new(
                network(9),
                node(8),
                identity.certificate().clone(),
                identity.fingerprint()
            ),
            Err(ControlTlsError::CertificateIdentityMismatch)
        ));
    }

    #[test]
    fn certificate_preface_is_bounded_and_matches_only_the_approved_pin() {
        let (client, server) = make_tls_pair();
        let client_allowed =
            PinnedControlTlsPeer::new(network(9), client.node(), client.fingerprint()).unwrap();
        let server_allowed =
            PinnedControlTlsPeer::new(network(9), server.node(), server.fingerprint()).unwrap();
        let (client_socket, server_socket) = stream_pair();
        let server_node = server.node();
        let server_certificate = server.certificate().clone();
        let server_private_key = server.private_key.clone_key();
        let server_thread = thread::spawn(move || {
            let server = LocalControlTlsIdentity::from_der(
                server_node,
                server_certificate,
                server_private_key,
            )
            .unwrap();
            let mut socket = server_socket;
            let pinned_client = exchange_control_tls_certificate(
                &mut socket,
                &server,
                network(9),
                &[client_allowed],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
            let server_config = server
                .server_config(network(9), std::slice::from_ref(&pinned_client))
                .unwrap();
            (pinned_client, server_config)
        });
        let mut socket = client_socket;
        let pinned_server = exchange_control_tls_certificate(
            &mut socket,
            &client,
            network(9),
            &[server_allowed],
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        let (pinned_client, _server_config) = server_thread.join().unwrap();
        assert_eq!(pinned_server.node(), server_node);
        assert_eq!(pinned_server.fingerprint(), server.fingerprint());
        assert_eq!(pinned_client.node(), client.node());
        assert_eq!(pinned_client.fingerprint(), client.fingerprint());

        // The remotely supplied candidate becomes a rustls trust input only
        // after the existing independently approved SPKI pin matches.
        let _client_config = client.client_config(network(9), &pinned_server).unwrap();
    }

    #[test]
    fn enrollment_candidate_requires_independent_fingerprint_confirmation() {
        let (client, server) = make_tls_pair();
        let (client_socket, server_socket) = stream_pair();
        let server_node = server.node();
        let server_certificate = server.certificate().clone();
        let server_private_key = server.private_key.clone_key();
        let server_thread = thread::spawn(move || {
            let server = LocalControlTlsIdentity::from_der(
                server_node,
                server_certificate,
                server_private_key,
            )
            .unwrap();
            exchange_control_tls_candidate(
                server_socket,
                &server,
                network(9),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap()
        });
        let client_candidate = exchange_control_tls_candidate(
            client_socket,
            &client,
            network(9),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        let server_candidate = server_thread.join().unwrap();

        assert_eq!(client_candidate.network(), network(9));
        assert_eq!(client_candidate.node(), server_node);
        assert_eq!(client_candidate.fingerprint(), server.fingerprint());
        assert_eq!(server_candidate.node(), client.node());
        assert_eq!(server_candidate.fingerprint(), client.fingerprint());

        let wrong_confirmation = SpkiSha256::from_verified_certificate_digest([99; 32]).unwrap();
        assert_eq!(
            client_candidate
                .clone()
                .confirm_fingerprint(wrong_confirmation)
                .unwrap_err(),
            ControlTlsError::FingerprintMismatch
        );
        let confirmed_server = client_candidate
            .confirm_fingerprint(server.fingerprint())
            .unwrap();
        let confirmed_client = server_candidate
            .confirm_fingerprint(client.fingerprint())
            .unwrap();
        assert_eq!(confirmed_server.peer_pin().node(), server_node);
        assert_eq!(confirmed_server.fingerprint(), server.fingerprint());

        // Enrollment confirmation is only a candidate pin. The actual
        // connection performs a fresh bounded preface and proves both keys.
        let (client_socket, server_socket) = stream_pair();
        let allowed_client = confirmed_client.peer_pin();
        let server_certificate_der = server.certificate().clone();
        let server_private_key = server.private_key.clone_key();
        let server_thread = thread::spawn(move || {
            let server = LocalControlTlsIdentity::from_der(
                server_node,
                server_certificate_der,
                server_private_key,
            )
            .unwrap();
            accept_pinned_control_tls(
                server_socket,
                &server,
                network(9),
                &[allowed_client],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap()
        });
        let (mut client_stream, _) = connect_pinned_control_tls(
            client_socket,
            &client,
            network(9),
            confirmed_server.peer_pin(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        let (mut server_stream, server_certificate) = server_thread.join().unwrap();
        assert_eq!(
            verify_control_tls_peer(&client_stream.connection, network(9), &confirmed_server)
                .unwrap()
                .fingerprint(),
            server.fingerprint()
        );
        assert_eq!(
            verify_control_tls_peer(&server_stream.connection, network(9), &server_certificate)
                .unwrap()
                .fingerprint(),
            client.fingerprint()
        );
        client_stream.close();
        server_stream.close();
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

    #[test]
    fn preface_factories_establish_only_pinned_tls13_connections() {
        let (client, server) = make_tls_pair();
        let client_pin =
            PinnedControlTlsPeer::new(network(9), client.node(), client.fingerprint()).unwrap();
        let server_pin =
            PinnedControlTlsPeer::new(network(9), server.node(), server.fingerprint()).unwrap();
        let (client_socket, server_socket) = stream_pair();
        let server_thread = thread::spawn(move || {
            let (stream, peer_certificate) = accept_pinned_control_tls(
                server_socket,
                &server,
                network(9),
                &[client_pin],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
            assert_eq!(peer_certificate.node(), node(1));
            assert_eq!(
                stream.connection.protocol_version(),
                Some(rustls::ProtocolVersion::TLSv1_3)
            );
        });
        let (stream, peer_certificate) = connect_pinned_control_tls(
            client_socket,
            &client,
            network(9),
            server_pin,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(peer_certificate.node(), node(2));
        assert_eq!(
            stream.connection.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        server_thread.join().unwrap();
    }

    #[test]
    fn persisted_identity_survives_restart_and_refuses_node_or_key_changes() {
        let temp = TempDirectory::new();
        let directory = temp.0.join("control-identity");
        let store = ControlTlsIdentityStore::new(&directory);
        let first = store.load_or_create(node(7)).unwrap();
        let restarted = ControlTlsIdentityStore::new(&directory)
            .load_or_create(node(7))
            .unwrap();

        assert_eq!(first.fingerprint(), restarted.fingerprint());
        assert!(store.load_or_create(node(8)).is_err());
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(directory.join(IDENTITY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn invalid_local_private_key_and_identity_records_fail_closed() {
        let first = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let second = LocalControlTlsIdentity::generate(node(2)).unwrap();
        assert!(matches!(
            LocalControlTlsIdentity::from_der(
                first.node(),
                first.certificate().clone(),
                second.private_key.clone_key()
            ),
            Err(ControlTlsError::KeyCertificateMismatch)
        ));

        let temp = TempDirectory::new();
        let directory = temp.0.join("control-identity");
        let store = ControlTlsIdentityStore::new(&directory);
        store.load_or_create(node(1)).unwrap();
        fs::write(directory.join(IDENTITY_FILE), b"malformed identity").unwrap();
        assert!(store.load_or_create(node(1)).is_err());
    }

    #[test]
    fn a_different_enrolled_fingerprint_is_rejected_before_tls() {
        let identity = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let other = LocalControlTlsIdentity::generate(node(2)).unwrap();
        assert!(matches!(
            PinnedControlTlsCertificate::new(
                network(9),
                node(1),
                identity.certificate().clone(),
                other.fingerprint()
            ),
            Err(ControlTlsError::FingerprintMismatch)
        ));
    }

    #[test]
    fn mutual_tls_uses_tls13_and_proves_both_private_keys() {
        let (client, server) = make_tls_pair();
        let pinned_client = PinnedControlTlsCertificate::new(
            network(9),
            client.node(),
            client.certificate().clone(),
            client.fingerprint(),
        )
        .unwrap();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (client_config, server_name) =
            client.client_config(network(9), &pinned_server).unwrap();
        let server_config = server
            .server_config(network(9), std::slice::from_ref(&pinned_client))
            .unwrap();
        let (client_socket, server_socket) = stream_pair();
        let server_node = server.node();
        let server_identity = server;

        let server_thread = thread::spawn(move || {
            let connection = rustls::ServerConnection::new(server_config).unwrap();
            let mut stream = rustls::StreamOwned::new(connection, server_socket);
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").unwrap();
            stream.flush().unwrap();
            let version = stream.conn.protocol_version();
            let mut connection = rustls::Connection::Server(stream.conn);
            let authenticated_peer =
                verify_control_tls_peer(&connection, network(9), &pinned_client).unwrap();
            let session_id = export_control_session_id(
                &mut connection,
                network(9),
                &server_identity,
                &authenticated_peer,
            )
            .unwrap();
            (version, session_id)
        });

        let connection = rustls::ClientConnection::new(client_config, server_name).unwrap();
        let mut stream = rustls::StreamOwned::new(connection, client_socket);
        stream.write_all(b"ping").unwrap();
        stream.flush().unwrap();
        let mut response = [0_u8; 4];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        assert_eq!(
            stream.conn.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        let mut connection = rustls::Connection::Client(stream.conn);
        let authenticated_peer =
            verify_control_tls_peer(&connection, network(9), &pinned_server).unwrap();
        assert_eq!(authenticated_peer.node(), server_node);
        let session_id =
            export_control_session_id(&mut connection, network(9), &client, &authenticated_peer)
                .unwrap();
        let pinned_server_other_network = PinnedControlTlsCertificate::new(
            network(10),
            pinned_server.node,
            pinned_server.certificate.clone(),
            pinned_server.fingerprint,
        )
        .unwrap();
        let authenticated_other_network =
            verify_control_tls_peer(&connection, network(10), &pinned_server_other_network)
                .unwrap();
        let other_network_id = export_control_session_id(
            &mut connection,
            network(10),
            &client,
            &authenticated_other_network,
        )
        .unwrap();
        assert_ne!(session_id, other_network_id);
        let (server_version, server_session_id) = server_thread.join().unwrap();
        assert_eq!(server_version, Some(rustls::ProtocolVersion::TLSv1_3));
        assert_eq!(session_id, server_session_id);
    }

    #[test]
    fn authenticated_hello_runs_inside_the_verified_tls_channel() {
        let (client, server) = make_tls_pair();
        let pinned_client = PinnedControlTlsCertificate::new(
            network(9),
            client.node(),
            client.certificate().clone(),
            client.fingerprint(),
        )
        .unwrap();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (client_config, server_name) =
            client.client_config(network(9), &pinned_server).unwrap();
        let server_config = server
            .server_config(network(9), std::slice::from_ref(&pinned_client))
            .unwrap();
        let (client_socket, server_socket) = stream_pair();
        client_socket
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        client_socket
            .set_write_timeout(Some(Duration::from_secs(17)))
            .unwrap();
        let server_thread = thread::spawn(move || {
            let mut stream = ControlTlsStream::server(
                rustls::ServerConnection::new(server_config).unwrap(),
                server_socket,
            );
            stream
                .complete_handshake_until(Instant::now() + Duration::from_secs(5))
                .unwrap();
            let authenticated = stream
                .exchange_authenticated_hello(
                    network(9),
                    &server,
                    &pinned_client,
                    hello_facts("server-test"),
                    Instant::now() + Duration::from_secs(5),
                )
                .unwrap();
            assert_eq!(authenticated.peer().node(), node(1));
            assert_eq!(authenticated.facts(), &hello_facts("client-test"));
            *authenticated.session_id()
        });

        let mut stream = ControlTlsStream::client(
            rustls::ClientConnection::new(client_config, server_name).unwrap(),
            client_socket,
        );
        stream
            .complete_handshake_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            stream.socket.read_timeout().unwrap(),
            Some(Duration::from_secs(15))
        );
        assert_eq!(
            stream.socket.write_timeout().unwrap(),
            Some(Duration::from_secs(17))
        );
        let authenticated = stream
            .exchange_authenticated_hello(
                network(9),
                &client,
                &pinned_server,
                hello_facts("client-test"),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(authenticated.peer().node(), node(2));
        assert_eq!(authenticated.facts(), &hello_facts("server-test"));
        assert_eq!(*authenticated.session_id(), server_thread.join().unwrap());
    }

    #[test]
    fn expired_tls_handshake_deadline_closes_its_socket() {
        let (client, server) = make_tls_pair();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (client_config, server_name) =
            client.client_config(network(9), &pinned_server).unwrap();
        let (client_socket, mut server_socket) = stream_pair();
        server_socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut stream = ControlTlsStream::client(
            rustls::ClientConnection::new(client_config, server_name).unwrap(),
            client_socket,
        );
        assert_eq!(
            stream.complete_handshake_until(Instant::now()),
            Err(ControlTlsError::HandshakeDeadlineExpired)
        );
        let mut byte = [0_u8; 1];
        assert_eq!(server_socket.read(&mut byte).unwrap(), 0);
    }

    #[test]
    fn tls_hello_activation_checks_durable_approval_before_ingress() {
        let (client, server) = make_tls_pair();
        let pinned_client = PinnedControlTlsCertificate::new(
            network(9),
            client.node(),
            client.certificate().clone(),
            client.fingerprint(),
        )
        .unwrap();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (client_config, server_name) =
            client.client_config(network(9), &pinned_server).unwrap();
        let server_config = server
            .server_config(network(9), std::slice::from_ref(&pinned_client))
            .unwrap();
        let (client_socket, server_socket) = stream_pair();
        let server_node = server.node();
        let client_node = client.node();
        let server_thread = thread::spawn(move || {
            let test_dir = TempDirectory::new();
            let mut stream = ControlTlsStream::server(
                rustls::ServerConnection::new(server_config).unwrap(),
                server_socket,
            );
            stream
                .complete_handshake_until(Instant::now() + Duration::from_secs(5))
                .unwrap();
            let authenticated = stream
                .exchange_authenticated_hello(
                    network(9),
                    &server,
                    &pinned_client,
                    hello_facts("server-test"),
                    Instant::now() + Duration::from_secs(5),
                )
                .unwrap();
            let trust = trust_for_peer(&test_dir.0, authenticated.peer());
            let mut active = stream
                .activate(authenticated, &server, &trust, active_deadline())
                .unwrap();
            assert_eq!(active.peer().node(), client_node);
            let first_batch = active.receive_next_at(10).unwrap();
            let ControlSessionRead::Frames(first_frames) = first_batch else {
                panic!("expected client frames before graceful close");
            };
            assert_eq!(first_frames[0].message, ControlMessage::Heartbeat);
            let goodbye_already_received = first_frames
                .iter()
                .skip(1)
                .any(|frame| frame.message == ControlMessage::Goodbye);
            assert!(first_frames.iter().all(|frame| matches!(
                frame.message,
                ControlMessage::Heartbeat | ControlMessage::Goodbye
            )));
            if !goodbye_already_received {
                assert!(matches!(
                    active.receive_next_at(10_000).unwrap(),
                    ControlSessionRead::Frames(frames)
                        if frames.len() == 1
                            && frames[0].message == ControlMessage::Goodbye
                ));
            }
            active
                .send_goodbye_until(Instant::now() + Duration::from_secs(5))
                .unwrap();
            assert_eq!(
                active.receive_next_at(20_000).unwrap(),
                ControlSessionRead::Closed
            );
        });

        let mut stream = ControlTlsStream::client(
            rustls::ClientConnection::new(client_config, server_name).unwrap(),
            client_socket,
        );
        stream
            .complete_handshake_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        let authenticated = stream
            .exchange_authenticated_hello(
                network(9),
                &client,
                &pinned_server,
                hello_facts("client-test"),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        let test_dir = TempDirectory::new();
        let trust = trust_for_peer(&test_dir.0, authenticated.peer());
        let mut active = stream
            .activate(authenticated, &client, &trust, active_deadline())
            .unwrap();
        active
            .send_heartbeat_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        active
            .send_goodbye_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        assert!(matches!(
            active.receive_next_at(10_000).unwrap(),
            ControlSessionRead::Frames(frames)
                if frames.len() == 1 && frames[0].message == ControlMessage::Goodbye
        ));
        assert_eq!(
            active.receive_next_at(20_000).unwrap(),
            ControlSessionRead::Closed
        );
        server_thread.join().unwrap();
        assert_eq!(server_node, node(2));
    }

    #[test]
    fn blocking_tls_read_cannot_renew_idle_deadline_using_stale_timestamp() {
        let (client, server) = make_tls_pair();
        let pinned_client = PinnedControlTlsCertificate::new(
            network(9),
            client.node(),
            client.certificate().clone(),
            client.fingerprint(),
        )
        .unwrap();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (client_config, server_name) =
            client.client_config(network(9), &pinned_server).unwrap();
        let server_config = server
            .server_config(network(9), std::slice::from_ref(&pinned_client))
            .unwrap();
        let (client_socket, server_socket) = stream_pair();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();

        let server_thread = thread::spawn(move || {
            let mut stream = ControlTlsStream::server(
                rustls::ServerConnection::new(server_config).unwrap(),
                server_socket,
            );
            stream
                .complete_handshake_until(Instant::now() + Duration::from_secs(5))
                .unwrap();
            let authenticated = stream
                .exchange_authenticated_hello(
                    network(9),
                    &server,
                    &pinned_client,
                    hello_facts("server-test"),
                    Instant::now() + Duration::from_secs(5),
                )
                .unwrap();
            let test_dir = TempDirectory::new();
            let trust = trust_for_peer(&test_dir.0, authenticated.peer());
            let mut active = stream
                .activate(authenticated, &server, &trust, active_deadline())
                .unwrap();
            // The active stage expires at 30_003 ms. A read that finishes at
            // the socket deadline must not return Idle at the stale timestamp.
            assert!(matches!(
                active.receive_next_at(29_903),
                Err(ControlSessionError::IdleDeadlineExpired)
            ));
            assert!(active.is_terminal());
            finished_tx.send(()).unwrap();
        });

        let mut stream = ControlTlsStream::client(
            rustls::ClientConnection::new(client_config, server_name).unwrap(),
            client_socket,
        );
        stream
            .complete_handshake_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        stream
            .exchange_authenticated_hello(
                network(9),
                &client,
                &pinned_server,
                hello_facts("client-test"),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        // Hold the TLS peer socket open until the server's read deadline.
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        server_thread.join().unwrap();
    }

    #[test]
    fn hello_evidence_cannot_be_reused_on_another_tls_connection() {
        let (client, server) = make_tls_pair();
        let pinned_client = PinnedControlTlsCertificate::new(
            network(9),
            client.node(),
            client.certificate().clone(),
            client.fingerprint(),
        )
        .unwrap();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (client_config, server_name) =
            client.client_config(network(9), &pinned_server).unwrap();
        let server_config = server
            .server_config(network(9), std::slice::from_ref(&pinned_client))
            .unwrap();
        let (first_client_socket, first_server_socket) = stream_pair();
        let first_server_config = Arc::clone(&server_config);
        let first_server = thread::spawn(move || {
            let mut stream = ControlTlsStream::server(
                rustls::ServerConnection::new(first_server_config).unwrap(),
                first_server_socket,
            );
            stream
                .complete_handshake_until(Instant::now() + Duration::from_secs(5))
                .unwrap();
            stream
                .exchange_authenticated_hello(
                    network(9),
                    &server,
                    &pinned_client,
                    hello_facts("server-test"),
                    Instant::now() + Duration::from_secs(5),
                )
                .unwrap();
        });
        let mut first_client = ControlTlsStream::client(
            rustls::ClientConnection::new(Arc::clone(&client_config), server_name.clone()).unwrap(),
            first_client_socket,
        );
        first_client
            .complete_handshake_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        let authenticated = first_client
            .exchange_authenticated_hello(
                network(9),
                &client,
                &pinned_server,
                hello_facts("client-test"),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        first_server.join().unwrap();
        drop(first_client);

        let trust_dir = TempDirectory::new();
        let trust = trust_for_peer(&trust_dir.0, authenticated.peer());
        let (second_client_socket, second_server_socket) = stream_pair();
        let second_server = thread::spawn(move || {
            let mut stream = ControlTlsStream::server(
                rustls::ServerConnection::new(server_config).unwrap(),
                second_server_socket,
            );
            stream
                .complete_handshake_until(Instant::now() + Duration::from_secs(5))
                .unwrap();
        });
        let mut second_client = ControlTlsStream::client(
            rustls::ClientConnection::new(client_config, server_name).unwrap(),
            second_client_socket,
        );
        second_client
            .complete_handshake_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        assert!(matches!(
            second_client.activate(authenticated, &client, &trust, active_deadline()),
            Err(ControlSessionError::Authentication(
                ControlTlsError::ExporterMismatch
            ))
        ));
        second_server.join().unwrap();
    }

    fn active_deadline() -> ConnectionDeadline {
        use blueroute_protocol::session::ConnectionStage;

        let mut deadline = ConnectionDeadline::new(0);
        deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
        deadline.advance(ConnectionStage::Hello, 2).unwrap();
        deadline.advance(ConnectionStage::Active, 3).unwrap();
        deadline
    }

    #[test]
    fn client_certificate_is_required_and_unknown_keys_fail() {
        let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
        let client = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let unknown_client = LocalControlTlsIdentity::generate(node(3)).unwrap();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (client_config, server_name) = unknown_client
            .client_config(network(9), &pinned_server)
            .unwrap();
        let server_config = server
            .server_config(
                network(9),
                &[PinnedControlTlsCertificate::new(
                    network(9),
                    client.node(),
                    client.certificate().clone(),
                    client.fingerprint(),
                )
                .unwrap()],
            )
            .unwrap();
        let (client_socket, server_socket) = stream_pair();
        let server_thread = thread::spawn(move || {
            let mut stream = rustls::StreamOwned::new(
                rustls::ServerConnection::new(server_config).unwrap(),
                server_socket,
            );
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte)
        });

        let mut stream = rustls::StreamOwned::new(
            rustls::ClientConnection::new(client_config, server_name).unwrap(),
            client_socket,
        );
        let _ = stream.write_all(b"x");
        assert!(server_thread.join().unwrap().is_err());
    }
}

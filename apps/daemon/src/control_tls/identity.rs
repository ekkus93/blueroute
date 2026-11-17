//! The local control-TLS identity: key generation, its persisted record format and `SecretFileStore` storage, and wiping key material on drop.

use super::certificate::{
    PinnedControlTlsCertificate, certificate_fingerprint, certificate_names_node, node_dns_name,
};
use super::{
    ControlTlsError, IDENTITY_FILE, IDENTITY_HEADER, MAX_CERTIFICATE_BYTES, MAX_PRIVATE_KEY_BYTES,
    TLS13_ONLY, is_zero_network, is_zero_node,
};
use blueroute_core::{CoreError, ErrorKind, NetworkId, NodeId, Secret};
use blueroute_linux::SecretFileStore;
use blueroute_protocol::peer_pin::SpkiSha256;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroize;

/// A local X.509 TLS identity. Callers should keep this value in daemon
/// lifetime memory and persist its private DER only through `SecretFileStore`.
pub struct LocalControlTlsIdentity {
    pub(super) node: NodeId,
    pub(super) certificate: CertificateDer<'static>,
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
        // Bound caller-supplied keys before cloning or parsing them.
        // The owner-only persisted format has the same size ceiling.
        if private_key.secret_der().is_empty()
            || private_key.secret_der().len() > MAX_PRIVATE_KEY_BYTES
        {
            return Err(ControlTlsError::InvalidKey);
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
        // Reject ambiguous identity-to-key mappings before constructing a TLS
        // verifier. A certificate may never authorize two logical peers, and
        // one logical peer may never have two concurrently trusted keys.
        let mut nodes = BTreeSet::new();
        let mut fingerprints = BTreeSet::new();
        let mut roots = RootCertStore::empty();
        for peer in peers {
            if !nodes.insert(peer.node) || !fingerprints.insert(peer.fingerprint) {
                return Err(ControlTlsError::InvalidIdentity);
            }
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

#[cfg(test)]
mod tests {

    use std::fs;

    use std::os::unix::fs::PermissionsExt;

    use std::thread;

    use super::super::test_support::{TempDirectory, make_tls_pair, network, node, stream_pair};
    use super::super::{ControlTlsError, IDENTITY_FILE, MAX_PRIVATE_KEY_BYTES};
    use super::*;
    use crate::control_tls::certificate::{
        PinnedControlTlsCertificate, PinnedControlTlsPeer, certificate_fingerprint,
        certificate_node_id,
    };
    use crate::control_tls::enrollment::{
        exchange_control_tls_candidate, exchange_control_tls_certificate,
    };
    use crate::control_tls::transport::{
        accept_pinned_control_tls, connect_pinned_control_tls, verify_control_tls_peer,
    };
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn certificate_parsers_reject_valid_der_with_trailing_untrusted_bytes() {
        let local = LocalControlTlsIdentity::generate(node(7)).unwrap();
        // The signed certificate is intact but the complete input is not
        // canonical DER. Both parsers must reject the extra ASN.1 value.
        let mut bytes = local.certificate().as_ref().to_vec();
        bytes.extend_from_slice(&[0x05, 0x00]);
        let trailing = CertificateDer::from(bytes);
        assert!(matches!(
            certificate_fingerprint(&trailing),
            Err(ControlTlsError::InvalidCertificate)
        ));
        assert!(matches!(
            certificate_node_id(&trailing),
            Err(ControlTlsError::InvalidCertificate)
        ));
        assert!(matches!(
            PinnedControlTlsCertificate::new(
                network(9),
                local.node(),
                trailing.clone(),
                local.fingerprint(),
            ),
            Err(ControlTlsError::InvalidCertificate)
        ));
        assert!(matches!(
            LocalControlTlsIdentity::from_der(
                local.node(),
                trailing,
                local.private_key.clone_key(),
            ),
            Err(ControlTlsError::InvalidCertificate)
        ));
    }

    #[test]
    fn local_tls_constructor_enforces_persisted_private_key_size_limits() {
        let local = LocalControlTlsIdentity::generate(node(7)).unwrap();
        let too_large = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(vec![
            0_u8;
            MAX_PRIVATE_KEY_BYTES
                + 1
        ]));
        assert!(matches!(
            LocalControlTlsIdentity::from_der(local.node(), local.certificate().clone(), too_large,),
            Err(ControlTlsError::InvalidKey)
        ));
        let empty = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(Vec::new()));
        assert!(matches!(
            LocalControlTlsIdentity::from_der(local.node(), local.certificate().clone(), empty),
            Err(ControlTlsError::InvalidKey)
        ));
        assert!(
            LocalControlTlsIdentity::from_der(
                local.node(),
                local.certificate().clone(),
                local.private_key.clone_key(),
            )
            .is_ok()
        );
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
}

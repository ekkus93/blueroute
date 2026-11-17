//! Pinned mutual TLS 1.3: connecting and accepting, verifying the peer against its approved pin, and the deadline-bounded TLS stream.

use super::certificate::{
    PinnedControlTlsCertificate, PinnedControlTlsPeer, certificate_fingerprint,
    certificate_names_node,
};
use super::enrollment::exchange_control_tls_certificate;
use super::identity::LocalControlTlsIdentity;
use super::session::{
    ActiveControlSession, AuthenticatedControlPeer, ControlSessionError, export_control_session_id,
};
use super::{ControlTlsError, is_zero_network, is_zero_node};
use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::control_io::{DeadlineControlRead, DeadlineControlWrite};
use blueroute_protocol::handshake::{HelloExchangeRequest, exchange_hello_claims_with_deadline};
use blueroute_protocol::peer_pin::SpkiSha256;
use blueroute_protocol::session::ConnectionDeadline;
use blueroute_protocol::session_writer::SessionControlWriter;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

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

/// Identity authenticated by the completed TLS handshake and matched to one
/// current enrolled certificate. Its fields cannot be supplied by callers.
#[derive(Clone, Debug)]
pub struct VerifiedControlTlsPeer {
    pub(super) network: NetworkId,
    pub(super) node: NodeId,
    pub(super) fingerprint: SpkiSha256,
}

/// A TLS-protected TCP stream. The TLS connection and its socket stay paired
/// so exporter material and Hello frames necessarily use the same channel.
pub struct ControlTlsStream {
    pub(super) connection: rustls::Connection,
    pub(super) socket: TcpStream,
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

    /// Bound TLS close-notify by the *same absolute deadline* as Goodbye.
    /// The frame writer restores socket timeouts, so without this guard a
    /// blocked close-notify flush could exceed the caller's shutdown budget.
    /// On error the owning active session closes the entire socket.
    pub(super) fn send_close_notify_until(&mut self, deadline: Instant) -> io::Result<()> {
        let expired =
            || io::Error::new(io::ErrorKind::TimedOut, "TLS close-notify deadline expired");
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(expired());
        }
        let original_timeout = self.socket.write_timeout()?;
        self.socket.set_write_timeout(Some(remaining))?;
        match &mut self.connection {
            rustls::Connection::Client(connection) => connection.send_close_notify(),
            rustls::Connection::Server(connection) => connection.send_close_notify(),
        }
        self.flush()?;
        if Instant::now() >= deadline {
            return Err(expired());
        }
        self.socket.shutdown(Shutdown::Write)?;
        self.socket.set_write_timeout(original_timeout)?;
        if Instant::now() >= deadline {
            return Err(expired());
        }
        Ok(())
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

pub(super) fn verify_control_tls_peer_identity(
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

#[cfg(test)]
mod tests {

    use std::io::{Read, Write};

    use std::thread;

    use super::super::ControlTlsError;
    use super::super::test_support::{make_tls_pair, network, node, stream_pair};
    use super::*;
    use crate::control_tls::certificate::{
        PinnedControlTlsCertificate, PinnedControlTlsPeer, certificate_fingerprint, node_dns_name,
    };
    use crate::control_tls::identity::LocalControlTlsIdentity;
    use crate::control_tls::session::export_control_session_id;
    use rcgen::CertificateParams;
    use rcgen::KeyPair;

    #[test]
    fn server_config_rejects_duplicate_node_ids_and_public_keys() {
        let server = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let first = LocalControlTlsIdentity::generate(node(2)).unwrap();
        let rotated = LocalControlTlsIdentity::generate(node(2)).unwrap();
        let pin = |identity: &LocalControlTlsIdentity| {
            PinnedControlTlsCertificate::new(
                network(9),
                identity.node(),
                identity.certificate().clone(),
                identity.fingerprint(),
            )
            .unwrap()
        };
        let first_pin = pin(&first);
        assert!(matches!(
            server.server_config(network(9), &[first_pin.clone(), pin(&rotated)]),
            Err(ControlTlsError::InvalidIdentity)
        ));
        assert!(matches!(
            server.server_config(network(9), &[first_pin.clone(), first_pin.clone()]),
            Err(ControlTlsError::InvalidIdentity)
        ));

        // Two valid, single-SAN certificates may use the same public key
        // while claiming different NodeIds. They must not share an allowlist.
        let key = KeyPair::generate().unwrap();
        let certificate_for = |node_id| {
            CertificateParams::new(vec![node_dns_name(node_id)])
                .unwrap()
                .self_signed(&key)
                .unwrap()
        };
        let cert_a = certificate_for(node(3));
        let cert_b = certificate_for(node(4));
        let fingerprint = certificate_fingerprint(cert_a.der()).unwrap();
        assert_eq!(fingerprint, certificate_fingerprint(cert_b.der()).unwrap());
        let alias_a = PinnedControlTlsCertificate::new(
            network(9),
            node(3),
            cert_a.der().clone(),
            fingerprint,
        )
        .unwrap();
        let alias_b = PinnedControlTlsCertificate::new(
            network(9),
            node(4),
            cert_b.der().clone(),
            fingerprint,
        )
        .unwrap();
        assert!(matches!(
            server.server_config(network(9), &[alias_a, alias_b]),
            Err(ControlTlsError::InvalidIdentity)
        ));

        let distinct = LocalControlTlsIdentity::generate(node(5)).unwrap();
        assert!(
            server
                .server_config(network(9), &[first_pin, pin(&distinct)])
                .is_ok()
        );
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

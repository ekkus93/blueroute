//! The authenticated control session over a verified TLS stream: the exporter-bound Hello, framed reads and writes, and session errors.

use super::identity::LocalControlTlsIdentity;
use super::transport::{
    ControlTlsStream, VerifiedControlTlsPeer, verify_control_tls_peer_identity,
};
use super::{ControlTlsError, is_zero_network};
use blueroute_core::NetworkId;
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::handshake::tls_control_exporter_context;
use blueroute_protocol::handshake::{TLS_CONTROL_EXPORTER_BYTES, TLS_CONTROL_EXPORTER_LABEL};
use blueroute_protocol::session_writer::SessionControlWriter;
use std::fmt;
use std::io::{self, Read};
use std::time::{Duration, Instant};

/// Peer identity and capability claims returned only after mutual TLS and the
/// exporter-bound Hello/HelloAck transcript both succeed.
#[derive(Debug)]
pub struct AuthenticatedControlPeer {
    pub(super) peer: VerifiedControlTlsPeer,
    pub(super) session_id: [u8; TLS_CONTROL_EXPORTER_BYTES],
    pub(super) facts: HelloFacts,
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
    pub(super) stream: ControlTlsStream,
    pub(super) ingress: crate::PinnedSessionIngress<'a>,
    pub(super) trust: &'a crate::DurablePeerTrustOperations,
    pub(super) writer: SessionControlWriter,
    pub(super) peer: VerifiedControlTlsPeer,
    pub(super) session_id: [u8; TLS_CONTROL_EXPORTER_BYTES],
    pub(super) facts: HelloFacts,
    pub(super) terminal: bool,
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
            || self.stream.send_close_notify_until(deadline).is_err()
        {
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::OutboundFailure);
        }
        Ok(())
    }

    /// Recheck durable approval without sending a frame. A daemon lifecycle
    /// supervisor can use this after trust changes to close revoked channels
    /// even when the peer is idle and no inbound frame is pending.
    pub(crate) fn revalidate_approval(&mut self) -> Result<(), ControlSessionError> {
        self.authorize_outbound()
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
        self.receive_next_with_budget_at(now_ms, Duration::MAX)
    }

    /// Poll authenticated TLS without monopolizing the daemon worker for
    /// the full peer idle timeout. Only complete replay-checked inbound
    /// frames renew the idle deadline; a short socket timeout does not.
    /// A read completing with bytes after its slice deadline fails closed.
    pub fn receive_next_with_budget_at(
        &mut self,
        now_ms: u64,
        max_wait: Duration,
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

        let idle_remaining = Duration::from_millis(expires_ms - now_ms);
        let timeout = idle_remaining.min(max_wait);
        if timeout.is_zero() {
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::IdleDeadlineExpired);
        }
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
        if read_finished >= read_deadline
            && !matches!(
                &read_result,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    )
            )
        {
            // A late plaintext completion must never refresh peer liveness.
            let _ = self.ingress.receive_verified_plaintext(&[], expires_ms);
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::IdleDeadlineExpired);
        }
        // A full-idle-budget socket timeout is terminal even when the OS
        // wakes a fraction of a millisecond before the requested timeout.
        // Rounding elapsed time down must never renew an expired peer lease.
        // Short bounded polls, by contrast, remain non-terminal.
        if timeout == idle_remaining
            && matches!(
                &read_result,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    )
            )
        {
            let _ = self.ingress.receive_verified_plaintext(&[], expires_ms);
            self.terminal = true;
            self.stream.close();
            return Err(ControlSessionError::IdleDeadlineExpired);
        }
        let elapsed_ms = read_finished.duration_since(read_started).as_millis();
        let elapsed_ms = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
        let observed_now_ms = if read_finished >= read_deadline && timeout == idle_remaining {
            // Millisecond rounding must not extend the true idle deadline.
            expires_ms
        } else {
            now_ms.saturating_add(elapsed_ms)
        };
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

    use blueroute_protocol::control::ControlMessage;

    use std::io::{Read, Write};

    use std::thread;

    use super::super::ControlTlsError;
    use super::super::test_support::{
        TempDirectory, active_deadline, hello_facts, make_tls_pair, network, node, stream_pair,
        trust_for_peer,
    };
    use super::*;
    use crate::control_tls::certificate::PinnedControlTlsCertificate;
    use crate::control_tls::transport::ControlTlsStream;
    use std::sync::Arc;

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
    fn malformed_post_hello_frames_on_mutual_tls_fail_closed_without_reusing_session() {
        use blueroute_protocol::control::{CONTROL_VERSION, ControlEnvelope, encode_control_frame};

        // All inputs traverse real TLS 1.3 mutual certificate checks, exporter
        // bound Hello/HelloAck, and durable approved-key ingress. Untrusted
        // application frames are injected only after those security gates.
        // Each new connection gets its own authorization and TLS exporter.
        for attack in 0..7 {
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
                let dir = TempDirectory::new();
                let trust = trust_for_peer(&dir.0, authenticated.peer());
                let active = stream
                    .activate(authenticated, &server, &trust, active_deadline())
                    .unwrap();
                let registry = Arc::new(std::sync::Mutex::new(
                    blueroute_protocol::session::ControlSessionRegistry::new(),
                ));
                let key = blueroute_protocol::session::PeerSessionKey {
                    network: network(9),
                    peer: node(1),
                };
                let mut registered =
                    crate::RegisteredControlSession::admit(active, Arc::clone(&registry)).unwrap();
                assert!(registry.lock().unwrap().get(key).is_some());
                assert!(
                    registered.receive_next_at(10).is_err(),
                    "hostile TLS plaintext was accepted, case {attack}"
                );
                assert!(registered.is_terminal(), "session survived case {attack}");
                assert!(
                    registry.lock().unwrap().get(key).is_none(),
                    "malformed input retained registry slot, case {attack}"
                );
                assert!(
                    registered.receive_next_at(11).is_err(),
                    "terminated session was reusable, case {attack}"
                );
                drop(registered);
                assert!(
                    registry.lock().unwrap().get(key).is_none(),
                    "stale owner drop resurrected a slot, case {attack}"
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
            let mut frame = ControlEnvelope {
                version: CONTROL_VERSION,
                network: network(9),
                sender: client.node(),
                session_id: *authenticated.session_id(),
                sequence: 3,
                message: ControlMessage::Heartbeat,
            };
            let hostile = match attack {
                0 => {
                    frame.network = network(8);
                    encode_control_frame(&frame).unwrap()
                }
                1 => {
                    frame.sender = node(3);
                    encode_control_frame(&frame).unwrap()
                }
                2 => {
                    frame.session_id = [0x7f; 16];
                    encode_control_frame(&frame).unwrap()
                }
                3 => {
                    frame.sequence = 2; // Replay a completed Hello/Ack slot.
                    encode_control_frame(&frame).unwrap()
                }
                4 => {
                    frame.message = ControlMessage::Hello {
                        challenge: [7; 16],
                        facts: hello_facts("replayed-hello"),
                    };
                    encode_control_frame(&frame).unwrap()
                }
                5 => {
                    let valid = encode_control_frame(&frame).unwrap();
                    let mut payload: serde_json::Value =
                        serde_json::from_slice(&valid[4..]).unwrap();
                    payload["critical_unknown"] = serde_json::json!(true);
                    let raw = serde_json::to_vec(&payload).unwrap();
                    let mut data = u32::try_from(raw.len()).unwrap().to_be_bytes().to_vec();
                    data.extend_from_slice(&raw);
                    data
                }
                6 => vec![0xff, 0xff, 0xff, 0xff],
                _ => unreachable!(),
            };
            stream
                .write_all(&hostile)
                .unwrap_or_else(|error| panic!("TLS write case {attack}: {error}"));
            stream.flush().unwrap();
            server_thread.join().unwrap();
        }
    }

    #[test]
    fn expired_close_notify_budget_rejects_before_flushing_and_preserves_timeout() {
        let (client, server) = make_tls_pair();
        let pinned_server = PinnedControlTlsCertificate::new(
            network(9),
            server.node(),
            server.certificate().clone(),
            server.fingerprint(),
        )
        .unwrap();
        let (config, server_name) = client.client_config(network(9), &pinned_server).unwrap();
        let (socket, mut peer) = stream_pair();
        let mut stream = ControlTlsStream::client(
            rustls::ClientConnection::new(config, server_name).unwrap(),
            socket,
        );
        let original_timeout = Some(Duration::from_secs(7));
        stream.socket.set_write_timeout(original_timeout).unwrap();
        let error = stream.send_close_notify_until(Instant::now()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(stream.socket.write_timeout().unwrap(), original_timeout);
        stream.close();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        assert_eq!(peer.read(&mut [0_u8; 1]).unwrap(), 0);
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
            let goodbye_at = if goodbye_already_received {
                // Heartbeat and Goodbye may be coalesced in one TLS read.
                10
            } else {
                assert!(matches!(
                    active.receive_next_at(10_000).unwrap(),
                    ControlSessionRead::Frames(frames)
                        if frames.len() == 1
                            && frames[0].message == ControlMessage::Goodbye
                ));
                10_000
            };
            active
                .send_goodbye_until(Instant::now() + Duration::from_secs(5))
                .unwrap();
            assert_eq!(
                active.receive_next_at(goodbye_at + 1_000).unwrap(),
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
            active.receive_next_at(11_000).unwrap(),
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
}

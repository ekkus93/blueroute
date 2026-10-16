//! Composed authenticated control transport for already-enrolled peers.
//!
//! Accepts an owned TCP socket only after checking durable (NetworkId, NodeId,
//! SPKI) authorization, proving possession via mutual TLS 1.3, and binding
//! Hello/HelloAck to the TLS exporter. The active ingress rechecks durable
//! authorization. This is a reusable daemon transport seam, not a PAN socket
//! listener, key-enrollment ceremony, or production JoinNetwork activation.

use std::net::{Shutdown, TcpStream};
use std::time::Instant;

use blueroute_core::NetworkId;
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::session::ConnectionDeadline;

use crate::{
    ActiveControlSession, ControlSessionError, DurablePeerTrustOperations, LocalControlTlsIdentity,
    PinnedControlTlsPeer, PinnedIngressError, accept_pinned_control_tls,
    connect_pinned_control_tls,
};

/// Caller-controlled stage deadlines and authenticated local capabilities.
/// `active_deadline` must already be in the protocol's Active stage.
pub struct ApprovedControlSetup {
    pub network: NetworkId,
    pub facts: HelloFacts,
    pub handshake_deadline: Instant,
    pub active_deadline: ConnectionDeadline,
}

fn deny(socket: &TcpStream, error: blueroute_core::CoreError) -> ControlSessionError {
    let _ = socket.shutdown(Shutdown::Both);
    ControlSessionError::Ingress(PinnedIngressError::Authorization(error))
}

/// Connect to one previously approved and enrolled peer. The candidate
/// preface is never treated as authorization: durable approval is checked
/// before the socket handshake and again on active session activation.
pub fn connect_approved_control_session<'a>(
    socket: TcpStream,
    local: &LocalControlTlsIdentity,
    expected: PinnedControlTlsPeer,
    trust: &'a DurablePeerTrustOperations,
    setup: ApprovedControlSetup,
) -> Result<ActiveControlSession<'a>, ControlSessionError> {
    if let Err(error) = trust.require_approved_pinned_tls_peer(
        setup.network,
        expected.node(),
        expected.fingerprint(),
    ) {
        return Err(deny(&socket, error));
    }
    let (mut stream, certificate) = connect_pinned_control_tls(
        socket,
        local,
        setup.network,
        expected,
        setup.handshake_deadline,
    )
    .map_err(ControlSessionError::Authentication)?;
    let authenticated = stream
        .exchange_authenticated_hello(
            setup.network,
            local,
            &certificate,
            setup.facts,
            setup.handshake_deadline,
        )
        .map_err(ControlSessionError::Authentication)?;
    stream.activate(authenticated, local, trust, setup.active_deadline)
}

/// Accept one inbound peer from a snapshot of independently enrolled pins.
/// Every supplied pin must still be authorized on disk before opening the
/// TLS handshake. Revocation is checked again when the session activates
/// and during every subsequent inbound/outbound frame operation.
pub fn accept_approved_control_session<'a>(
    socket: TcpStream,
    local: &LocalControlTlsIdentity,
    allowed: &[PinnedControlTlsPeer],
    trust: &'a DurablePeerTrustOperations,
    setup: ApprovedControlSetup,
) -> Result<ActiveControlSession<'a>, ControlSessionError> {
    for peer in allowed {
        if let Err(error) = trust.require_approved_pinned_tls_peer(
            setup.network,
            peer.node(),
            peer.fingerprint(),
        ) {
            return Err(deny(&socket, error));
        }
    }
    let (mut stream, certificate) = accept_pinned_control_tls(
        socket,
        local,
        setup.network,
        allowed,
        setup.handshake_deadline,
    )
    .map_err(ControlSessionError::Authentication)?;
    let authenticated = stream
        .exchange_authenticated_hello(
            setup.network,
            local,
            &certificate,
            setup.facts,
            setup.handshake_deadline,
        )
        .map_err(ControlSessionError::Authentication)?;
    stream.activate(authenticated, local, trust, setup.active_deadline)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    use blueroute_core::{
        DisplayName, MembershipRegistry, MembershipState, NetworkMembership, NodeId,
    };
    use blueroute_linux::NetworkMembershipStore;
    use blueroute_protocol::control::ControlMessage;
    use blueroute_protocol::peer_pin::SpkiSha256;
    use blueroute_protocol::session::ConnectionStage;

    use super::*;
    use crate::{PeerPinStore, PeerTrustOperations};
    use crate::control_tls::ControlSessionRead;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "blueroute-approved-transport-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([9; 16])
    }

    fn node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn trust_for(
        path: &Path,
        remote: NodeId,
        remote_fingerprint: SpkiSha256,
        enroll: bool,
    ) -> DurablePeerTrustOperations {
        let membership_store = NetworkMembershipStore::new(path.join("memberships"));
        let mut membership =
            NetworkMembership::new(network(), DisplayName::new("Test network").unwrap());
        membership.state = MembershipState::Member;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        membership_store.save(&registry).unwrap();
        let trust = DurablePeerTrustOperations::with_peer_pin_store(
            membership_store,
            PeerPinStore::new(path.join("peer-pins")),
        );
        futures_lite::future::block_on(trust.trust_peer(remote)).unwrap();
        if enroll {
            // Synthetic approval is permitted only in this test; production
            // enrollment requires an independently confirmed TLS identity.
            trust
                .enroll_verified_key(
                    network(),
                    remote,
                    remote_fingerprint,
                    remote_fingerprint,
                )
                .unwrap();
        }
        trust
    }

    fn setup() -> ApprovedControlSetup {
        let mut active_deadline = ConnectionDeadline::new(0);
        active_deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
        active_deadline.advance(ConnectionStage::Hello, 2).unwrap();
        active_deadline.advance(ConnectionStage::Active, 3).unwrap();
        ApprovedControlSetup {
            network: network(),
            facts: HelloFacts {
                software_version: "approved-transport-test".to_owned(),
                capabilities: blueroute_core::NodeCapabilities::default(),
                segment: None,
                direct_neighbors: Vec::new(),
            },
            handshake_deadline: Instant::now() + Duration::from_secs(5),
            active_deadline,
        }
    }

    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn non_enrolled_peer_cannot_begin_control_tls() {
        let local = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let peer = LocalControlTlsIdentity::generate(node(2)).unwrap();
        let dir = TestDirectory::new();
        let trust = trust_for(&dir.0, peer.node(), peer.fingerprint(), false);
        let expected =
            PinnedControlTlsPeer::new(network(), peer.node(), peer.fingerprint()).unwrap();
        let (socket, mut remote) = socket_pair();
        assert!(matches!(
            connect_approved_control_session(socket, &local, expected, &trust, setup()),
            Err(ControlSessionError::Ingress(PinnedIngressError::Authorization(_)))
        ));
        remote.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(remote.read(&mut byte).unwrap(), 0);
    }

    #[test]
    fn approved_peers_exchange_authenticated_frames_over_owned_tcp() {
        let client = LocalControlTlsIdentity::generate(node(1)).unwrap();
        let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
        let client_pin =
            PinnedControlTlsPeer::new(network(), client.node(), client.fingerprint()).unwrap();
        let server_pin =
            PinnedControlTlsPeer::new(network(), server.node(), server.fingerprint()).unwrap();
        let (client_socket, server_socket) = socket_pair();
        let server_thread = thread::spawn(move || {
            let dir = TestDirectory::new();
            let trust = trust_for(&dir.0, client_pin.node(), client_pin.fingerprint(), true);
            let mut session = accept_approved_control_session(
                server_socket,
                &server,
                &[client_pin],
                &trust,
                setup(),
            )
            .unwrap();
            assert_eq!(session.peer().node(), node(1));
            assert!(matches!(
                session.receive_next_at(10).unwrap(),
                ControlSessionRead::Frames(frames)
                    if frames.len() == 1 && frames[0].message == ControlMessage::Heartbeat
            ));
        });

        let dir = TestDirectory::new();
        let trust = trust_for(&dir.0, server_pin.node(), server_pin.fingerprint(), true);
        let mut session = connect_approved_control_session(
            client_socket,
            &client,
            server_pin,
            &trust,
            setup(),
        )
        .unwrap();
        assert_eq!(session.peer().node(), node(2));
        session
            .send_heartbeat_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        server_thread.join().unwrap();
    }
}

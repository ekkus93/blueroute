//! Real localhost mutual-TLS regression for generation-scoped PAN loss callbacks.
//! This is software-only: it does not prove a live daemon PAN listener or JoinNetwork.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use blueroute_core::{
    DisplayName, MembershipRegistry, MembershipState, NetworkId, NetworkMembership, NodeId,
};
use blueroute_daemon::{
    ApprovedControlSetup, DurablePeerTrustOperations, LocalControlTlsIdentity, PeerPinStore,
    PeerTrustOperations, PinnedControlTlsPeer, accept_registered_from_durable_approval,
    connect_registered_control_session,
};
use blueroute_linux::NetworkMembershipStore;
use blueroute_protocol::control::{ControlMessage, HelloFacts};
use blueroute_protocol::peer_pin::SpkiSha256;
use blueroute_protocol::session::{
    ConnectionDeadline, ConnectionStage, ControlSessionRegistry, PeerSessionKey,
};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "blueroute-generation-cancel-{}-{id}",
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

fn node(id: u8) -> NodeId {
    NodeId::from_bytes([id; 16])
}

fn trust_for(path: &Path, peer: NodeId, fingerprint: SpkiSha256) -> DurablePeerTrustOperations {
    let membership_store = NetworkMembershipStore::new(path.join("memberships"));
    let mut membership =
        NetworkMembership::new(network(), DisplayName::new("Generation test").unwrap());
    membership.state = MembershipState::Member;
    let mut registry = MembershipRegistry::default();
    registry.remember_network(membership);
    membership_store.save(&registry).unwrap();
    let trust = DurablePeerTrustOperations::with_peer_pin_store(
        membership_store,
        PeerPinStore::new(path.join("peer-pins")),
    );
    futures_lite::future::block_on(trust.trust_peer(peer)).unwrap();
    // Test-only synthetic independent confirmation; no production enrollment.
    trust
        .enroll_verified_key(network(), peer, fingerprint, fingerprint)
        .unwrap();
    trust
}

fn setup() -> ApprovedControlSetup {
    let mut active_deadline = ConnectionDeadline::new(0);
    active_deadline
        .advance(ConnectionStage::TlsHandshake, 1)
        .unwrap();
    active_deadline.advance(ConnectionStage::Hello, 2).unwrap();
    active_deadline.advance(ConnectionStage::Active, 3).unwrap();
    ApprovedControlSetup {
        network: network(),
        facts: HelloFacts {
            software_version: "generation-cancel-test".to_owned(),
            capabilities: blueroute_core::NodeCapabilities::default(),
            segment: None,
            direct_neighbors: Vec::new(),
        },
        handshake_deadline: Instant::now() + Duration::from_secs(5),
        active_deadline,
    }
}

#[test]
fn stale_loss_generation_preserves_authenticated_tls_and_current_loss_closes_it() {
    let client = LocalControlTlsIdentity::generate(node(1)).unwrap();
    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let client_pin =
        PinnedControlTlsPeer::new(network(), client.node(), client.fingerprint()).unwrap();
    let server_pin =
        PinnedControlTlsPeer::new(network(), server.node(), server.fingerprint()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client_socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server_socket, _) = listener.accept().unwrap();
    let (closed_tx, closed_rx) = mpsc::channel();

    let server_thread = thread::spawn(move || {
        let dir = TestDirectory::new();
        let trust = trust_for(&dir.0, client_pin.node(), client_pin.fingerprint());
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut session = accept_registered_from_durable_approval(
            server_socket,
            &server,
            &trust,
            setup(),
            Arc::clone(&registry),
        )
        .unwrap();
        let key = PeerSessionKey {
            network: network(),
            peer: client_pin.node(),
        };
        let current = *session.session_id();
        let mut stale = current;
        stale[0] ^= 0x80;
        assert!(!session.cancel_if_current(stale));
        assert!(!session.is_terminal());
        assert_eq!(
            registry.lock().unwrap().get(key).unwrap().session_id,
            current
        );

        // The live TLS session still processes an authenticated frame after
        // the stale callback, proving it was not merely left in the registry.
        let received = session.receive_next_at(10).unwrap();
        assert!(matches!(received, blueroute_daemon::ControlSessionRead::Frames(frames)
            if frames.len() == 1 && frames[0].message == ControlMessage::Heartbeat));
        assert!(session.cancel_if_current(current));
        assert!(session.is_terminal());
        assert!(registry.lock().unwrap().get(key).is_none());
        assert!(session.cancel_if_current(current)); // idempotent
        closed_tx.send(()).unwrap();
        // Keep the cancelled owner alive until the client sees EOF.
        thread::sleep(Duration::from_millis(100));
    });

    let dir = TestDirectory::new();
    let trust = trust_for(&dir.0, server_pin.node(), server_pin.fingerprint());
    let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
    let mut session = connect_registered_control_session(
        client_socket, &client, server_pin, &trust, setup(), registry,
    ).unwrap();
    session.send_heartbeat_until(Instant::now() + Duration::from_secs(5)).unwrap();
    closed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // Abrupt socket shutdown is not a protocol Goodbye.
    assert!(session.receive_next_at(11).is_err());
    server_thread.join().unwrap();
}

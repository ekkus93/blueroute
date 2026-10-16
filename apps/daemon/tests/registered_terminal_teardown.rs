//! Regression: a failed authenticated send must close TCP before a peer
//! registry slot can be reused, even while the Rust owner is still alive.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use blueroute_core::{
    DisplayName, MembershipRegistry, MembershipState, NetworkId, NetworkMembership,
    NodeCapabilities, NodeId,
};
use blueroute_daemon::{
    ApprovedControlSetup, DurablePeerTrustOperations, LocalControlTlsIdentity, PeerPinStore,
    PeerTrustOperations, PinnedControlTlsPeer, accept_registered_control_session,
    connect_registered_control_session,
};
use blueroute_linux::NetworkMembershipStore;
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::session::{
    ConnectionDeadline, ConnectionStage, ControlSessionRegistry, PeerSessionKey,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "blueroute-registered-teardown-{}-{id}",
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
    fingerprint: blueroute_protocol::peer_pin::SpkiSha256,
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
    // Synthetic independent fingerprint confirmation is test-only.
    trust
        .enroll_verified_key(network(), remote, fingerprint, fingerprint)
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
            software_version: "registered-teardown-test".to_owned(),
            capabilities: NodeCapabilities::default(),
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
fn terminal_send_closes_tcp_and_retires_slot_without_waiting_for_owner_drop() {
    let client = LocalControlTlsIdentity::generate(node(1)).unwrap();
    let server = LocalControlTlsIdentity::generate(node(2)).unwrap();
    let client_pin =
        PinnedControlTlsPeer::new(network(), client.node(), client.fingerprint()).unwrap();
    let server_pin =
        PinnedControlTlsPeer::new(network(), server.node(), server.fingerprint()).unwrap();
    let (client_socket, server_socket) = socket_pair();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();

    let server_thread = thread::spawn(move || {
        let dir = TestDirectory::new();
        let trust = trust_for(&dir.0, client_pin.node(), client_pin.fingerprint());
        let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
        let mut session = accept_registered_control_session(
            server_socket,
            &server,
            &[client_pin],
            &trust,
            setup(),
            registry,
        )
        .unwrap();
        ready_tx.send(()).unwrap();
        // No Goodbye was sent. A terminal send must shut down TCP while the
        // client-side RegisteredControlSession remains in scope.
        session.receive_next_at(10).is_err()
    });

    let dir = TestDirectory::new();
    let trust = trust_for(&dir.0, server_pin.node(), server_pin.fingerprint());
    let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
    let mut session = connect_registered_control_session(
        client_socket,
        &client,
        server_pin,
        &trust,
        setup(),
        Arc::clone(&registry),
    )
    .unwrap();
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let key = PeerSessionKey {
        network: network(),
        peer: node(2),
    };
    assert!(registry.lock().unwrap().get(key).is_some());
    assert!(
        session
            .send_heartbeat_until(Instant::now() - Duration::from_millis(1))
            .is_err()
    );
    assert!(session.is_terminal());
    assert!(registry.lock().unwrap().get(key).is_none());
    assert!(server_thread.join().unwrap());
    drop(session);
}

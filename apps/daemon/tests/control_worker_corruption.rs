//! P7-004: real daemon worker must not trust a stale NAP when durable state is corrupt.
//! Localhost/fake-backend software test only; not Bluetooth hardware acceptance.
use std::net::{SocketAddrV4, TcpListener};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use blueroute_core::{
    DisplayName, MembershipRegistry, MembershipState, NetworkId, NetworkMembership,
    NodeCapabilities, NodeId,
};
use blueroute_daemon::{
    DurablePeerTrustOperations, LocalControlTlsIdentity, NetworkOperationFuture, NetworkOperations,
    ObservedControlListener, spawn_daemon_control_worker,
};
use blueroute_linux::NetworkMembershipStore;
use blueroute_protocol::NetworkSummary;

fn save_host(path: &Path) {
    let mut host = NetworkMembership::new(
        NetworkId::from_bytes([7; 16]),
        DisplayName::new("Corruption test").unwrap(),
    );
    host.state = MembershipState::Member;
    let mut registry = MembershipRegistry::default();
    registry.remember_network(host);
    NetworkMembershipStore::new(path.to_path_buf())
        .save(&registry)
        .unwrap();
}

#[derive(Default)]
struct StaleNap {
    endpoint: Mutex<Option<SocketAddrV4>>,
    binds: AtomicUsize,
}

impl NetworkOperations for StaleNap {
    fn bind_guarded_control_listener(
        &self,
    ) -> NetworkOperationFuture<'_, Option<ObservedControlListener>> {
        Box::pin(async move {
            let socket = TcpListener::bind("127.0.0.1:0").unwrap();
            let std::net::SocketAddr::V4(endpoint) = socket.local_addr().unwrap() else {
                unreachable!("loopback IPv4")
            };
            *self.endpoint.lock().unwrap() = Some(endpoint);
            self.binds.fetch_add(1, Ordering::SeqCst);
            Ok(Some(ObservedControlListener::new(
                NetworkId::from_bytes([7; 16]),
                endpoint,
                socket,
            )?))
        })
    }

    fn observed_owned_control_endpoint(&self) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
        Box::pin(async move { Ok(*self.endpoint.lock().unwrap()) })
    }

    fn create_network(&self, _: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        Box::pin(async { unreachable!("not a creation test") })
    }

    fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("not a leave test") })
    }

    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>> {
        Box::pin(async { unreachable!("not a discovery test") })
    }

    fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("not a discovery test") })
    }

    fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("not a discovery test") })
    }
}

fn wait_for(mut condition: impl FnMut() -> bool, why: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "{why}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn corrupt_membership_blocks_worker_startup_and_rebind_until_repair() {
    let path = std::env::temp_dir().join(format!(
        "blueroute-worker-corrupt-durable-{}",
        std::process::id()
    ));
    save_host(&path);
    std::fs::write(&path, b"invalid durable membership").unwrap();
    assert!(NetworkMembershipStore::new(path.clone()).load().is_err());

    let backend = Arc::new(StaleNap::default());
    let trust = Arc::new(DurablePeerTrustOperations::new(
        NetworkMembershipStore::new(path.clone()),
    ));
    let local = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
    let worker = spawn_daemon_control_worker(
        NetworkMembershipStore::new(path.clone()),
        backend.clone(),
        trust,
        local,
        NodeCapabilities::default(),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        backend.binds.load(Ordering::SeqCst),
        0,
        "corrupt durable state must deny startup ingress even with stale NAP"
    );

    save_host(&path);
    wait_for(
        || backend.binds.load(Ordering::SeqCst) > 0,
        "valid membership should allow a fresh listener",
    );
    let endpoint = backend.endpoint.lock().unwrap().unwrap();
    assert!(TcpListener::bind(endpoint).is_err());

    // A later on-disk corruption must retire the live listener and prevent
    // rebinds even though the fake backend continues reporting ownership.
    std::fs::write(&path, b"invalid durable membership again").unwrap();
    assert!(NetworkMembershipStore::new(path.clone()).load().is_err());
    wait_for(
        || TcpListener::bind(endpoint).is_ok(),
        "corruption must close the active TCP listener",
    );
    let count = backend.binds.load(Ordering::SeqCst);
    thread::sleep(Duration::from_millis(2300));
    assert_eq!(backend.binds.load(Ordering::SeqCst), count);
    assert!(
        NetworkMembershipStore::new(path.clone()).load().is_err(),
        "worker must not silently repair or discard corrupt state"
    );

    save_host(&path);
    wait_for(
        || backend.binds.load(Ordering::SeqCst) > count,
        "repaired durable membership should allow a fresh listener",
    );
    let replacement = backend.endpoint.lock().unwrap().unwrap();
    assert!(TcpListener::bind(replacement).is_err());
    worker.shutdown().unwrap();
    assert!(TcpListener::bind(replacement).is_ok());
}

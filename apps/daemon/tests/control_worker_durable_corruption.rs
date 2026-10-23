//! P7-004 integration regression: malformed durable NAP ownership fails closed.
//! Loopback/fake-backend evidence only; no physical Bluetooth is exercised.

use std::net::{SocketAddrV4, TcpListener};
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
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let std::net::SocketAddr::V4(endpoint) = listener.local_addr().unwrap() else {
                unreachable!("loopback test uses IPv4");
            };
            *self.endpoint.lock().unwrap() = Some(endpoint);
            self.binds.fetch_add(1, Ordering::SeqCst);
            Ok(Some(ObservedControlListener::new(
                NetworkId::from_bytes([7; 16]),
                endpoint,
                listener,
            )?))
        })
    }

    fn observed_owned_control_endpoint(&self) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
        Box::pin(async move { Ok(*self.endpoint.lock().unwrap()) })
    }

    fn create_network(&self, _: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        Box::pin(async { unreachable!("control worker must not create a network") })
    }

    fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("control worker must not leave a network") })
    }

    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<blueroute_protocol::NetworkSummary>> {
        Box::pin(async { unreachable!("control worker must not list networks") })
    }

    fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("control worker must not start discovery") })
    }

    fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("control worker must not stop discovery") })
    }
}

fn save_host(path: &std::path::Path) {
    let mut network = NetworkMembership::new(
        NetworkId::from_bytes([7; 16]),
        DisplayName::new("Test host").unwrap(),
    );
    network.state = MembershipState::Member;
    let mut registry = MembershipRegistry::default();
    registry.remember_network(network);
    NetworkMembershipStore::new(path.to_path_buf())
        .save(&registry)
        .unwrap();
}

#[test]
fn malformed_durable_host_retires_listener_and_requires_explicit_repair() {
    let path = std::env::temp_dir().join(format!(
        "blueroute-worker-corrupt-integration-{}",
        std::process::id()
    ));
    save_host(&path);
    let backend = Arc::new(StaleNap::default());
    let trust = Arc::new(DurablePeerTrustOperations::new(
        NetworkMembershipStore::new(path.clone()),
    ));
    let identity = LocalControlTlsIdentity::generate(NodeId::from_bytes([8; 16])).unwrap();
    let worker = spawn_daemon_control_worker(
        NetworkMembershipStore::new(path.clone()),
        backend.clone(),
        trust,
        identity,
        NodeCapabilities::default(),
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let original = loop {
        if let Some(address) = *backend.endpoint.lock().unwrap() {
            break address;
        }
        assert!(
            Instant::now() < deadline,
            "worker must bind the initial NAP"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert!(TcpListener::bind(original).is_err());

    // The fake backend continues claiming NAP ownership after corruption.
    // It cannot substitute for the owner-only durable membership record.
    let corrupt = b"malformed membership data";
    std::fs::write(&path, corrupt).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Ok(probe) = TcpListener::bind(original) {
            drop(probe);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "malformed durable membership must close the live TCP listener"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let binds = backend.binds.load(Ordering::SeqCst);
    thread::sleep(Duration::from_millis(2300));
    assert_eq!(
        backend.binds.load(Ordering::SeqCst),
        binds,
        "stale backend ownership cannot rebind after corruption"
    );
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);

    // Only an explicit durable-store repair can enable a new guarded bind.
    save_host(&path);
    let deadline = Instant::now() + Duration::from_secs(4);
    while backend.binds.load(Ordering::SeqCst) == binds {
        assert!(Instant::now() < deadline, "restored host must rebind");
        thread::sleep(Duration::from_millis(10));
    }
    let replacement = backend.endpoint.lock().unwrap().unwrap();
    assert!(TcpListener::bind(replacement).is_err());
    worker.shutdown().unwrap();
    assert!(TcpListener::bind(replacement).is_ok());
}

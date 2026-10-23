//! P7-004: an on-disk host-network switch during NAP bind must not leave
//! a listener serving a network whose durable ownership has already changed.
//! Loopback/fake backend only; never evidence of Bluetooth acceptance.

use std::net::{SocketAddrV4, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

fn save_host(path: &Path, network: NetworkId) {
    let mut host = NetworkMembership::new(network, DisplayName::new("Bind race test").unwrap());
    host.state = MembershipState::Member;
    let mut registry = MembershipRegistry::default();
    registry.remember_network(host);
    NetworkMembershipStore::new(path.to_path_buf())
        .save(&registry)
        .unwrap();
}

struct SwitchingNap {
    path: PathBuf,
    switch_on_first_bind: AtomicBool,
    endpoint: Mutex<Option<SocketAddrV4>>,
    binds: AtomicUsize,
}

impl NetworkOperations for SwitchingNap {
    fn bind_guarded_control_listener(
        &self,
    ) -> NetworkOperationFuture<'_, Option<ObservedControlListener>> {
        Box::pin(async move {
            let socket = TcpListener::bind("127.0.0.1:0").unwrap();
            let std::net::SocketAddr::V4(endpoint) = socket.local_addr().unwrap() else {
                unreachable!("IPv4 test socket")
            };
            *self.endpoint.lock().unwrap() = Some(endpoint);
            self.binds.fetch_add(1, Ordering::SeqCst);
            if self.switch_on_first_bind.swap(false, Ordering::SeqCst) {
                // Inject an actual durable state change *after* the daemon's
                // pre-bind check and while the backend still owns the socket.
                save_host(&self.path, NetworkId::from_bytes([9; 16]));
            }
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

fn wait_for(mut ready: impl FnMut() -> bool, context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "{context}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn durable_network_switch_during_bind_rejects_stale_listener_then_recovers() {
    let path = std::env::temp_dir().join(format!(
        "blueroute-worker-bind-network-switch-{}",
        std::process::id()
    ));
    let original = NetworkId::from_bytes([7; 16]);
    let replaced = NetworkId::from_bytes([9; 16]);
    save_host(&path, original);
    let backend = Arc::new(SwitchingNap {
        path: path.clone(),
        switch_on_first_bind: AtomicBool::new(true),
        endpoint: Mutex::new(None),
        binds: AtomicUsize::new(0),
    });
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

    wait_for(
        || backend.binds.load(Ordering::SeqCst) > 0,
        "fake backend did not attempt the guarded bind",
    );
    assert_eq!(
        blueroute_daemon::current_network(
            &NetworkMembershipStore::new(path.clone()).load().unwrap()
        )
        .unwrap(),
        Some(replaced),
        "bind callback must switch the durable host network",
    );
    let raced_endpoint = backend.endpoint.lock().unwrap().unwrap();
    wait_for(
        || TcpListener::bind(raced_endpoint).is_ok(),
        "stale network's TCP listener must be closed",
    );

    // The backend still reports a stale NAP for network 7; only restoring
    // durable network 7 permits the corresponding listener to survive.
    save_host(&path, original);
    let count = backend.binds.load(Ordering::SeqCst);
    wait_for(
        || backend.binds.load(Ordering::SeqCst) > count,
        "restoring matching durable membership must allow another bind",
    );
    let recovered = backend.endpoint.lock().unwrap().unwrap();
    wait_for(
        || TcpListener::bind(recovered).is_err(),
        "recovered listener must be retained while it owns the network",
    );
    worker.shutdown().unwrap();
    assert!(TcpListener::bind(recovered).is_ok());
}

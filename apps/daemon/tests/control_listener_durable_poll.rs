//! P7-004 real durable-membership and guarded endpoint polling composition.
//!
//! Loopback-only software tests: no independent enrollment, live PAN,
//! authenticated daemon TLS supervision, or production JoinNetwork evidence.

use std::fs;
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use blueroute_core::{
    CoreError, DisplayName, ErrorKind, MembershipRegistry, MembershipState, NetworkId,
    NetworkMembership,
};
use blueroute_daemon::{NetworkOperationFuture, NetworkOperations, ObservedControlListener};
use blueroute_linux::NetworkMembershipStore;
use blueroute_protocol::NetworkSummary;

static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

fn network(value: u8) -> NetworkId {
    NetworkId::from_bytes([value; 16])
}

fn member_store(label: &str) -> (PathBuf, NetworkMembershipStore) {
    let root = std::env::temp_dir().join(format!(
        "blueroute-durable-listener-{label}-{}-{}",
        std::process::id(),
        NEXT_TEST.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(&root).unwrap();
    let store = NetworkMembershipStore::new(root.join("membership"));
    let mut membership =
        NetworkMembership::new(network(7), DisplayName::new("NAP poll test").unwrap());
    membership.state = MembershipState::Member;
    let mut registry = MembershipRegistry::default();
    registry.remember_network(membership);
    store.save(&registry).unwrap();
    (root, store)
}

fn listener() -> (ObservedControlListener, SocketAddrV4) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let SocketAddr::V4(endpoint) = listener.local_addr().unwrap() else {
        panic!("loopback listener must be IPv4");
    };
    (
        ObservedControlListener::new(network(7), endpoint, listener).unwrap(),
        endpoint,
    )
}

#[derive(Clone, Copy)]
enum ObservationMode {
    Stable,
    RevokeDuringSecondObservation,
    BackendFailure,
    ReplaceNetworkDuringSecondObservation,
    CorruptDuringSecondObservation,
    EndpointDriftDuringSecondObservation,
}

struct Probe {
    endpoint: SocketAddrV4,
    membership_path: PathBuf,
    mode: ObservationMode,
    calls: AtomicUsize,
}

impl NetworkOperations for Probe {
    fn observed_owned_control_endpoint(&self) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            match self.mode {
                ObservationMode::RevokeDuringSecondObservation if call == 2 => {
                    NetworkMembershipStore::new(self.membership_path.clone())
                        .save(&MembershipRegistry::default())?;
                }
                ObservationMode::BackendFailure => {
                    return Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "injected trusted endpoint backend failure",
                    ));
                }
                ObservationMode::ReplaceNetworkDuringSecondObservation if call == 2 => {
                    let mut replacement = NetworkMembership::new(
                        network(8),
                        DisplayName::new("replacement").unwrap(),
                    );
                    replacement.state = MembershipState::Member;
                    let mut registry = MembershipRegistry::default();
                    registry.remember_network(replacement);
                    NetworkMembershipStore::new(self.membership_path.clone()).save(&registry)?;
                }
                ObservationMode::CorruptDuringSecondObservation if call == 2 => {
                    fs::write(&self.membership_path, b"BLUEROUTE_MEMBERSHIP_V999\n").unwrap();
                }
                _ => {}
            }
            let endpoint = match self.mode {
                ObservationMode::EndpointDriftDuringSecondObservation if call == 2 => {
                    SocketAddrV4::new(*self.endpoint.ip(), self.endpoint.port() ^ 1)
                }
                _ => self.endpoint,
            };
            Ok(Some(endpoint))
        })
    }

    fn create_network(&self, _: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        Box::pin(async { unreachable!("test only observes endpoints") })
    }
    fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("test only observes endpoints") })
    }
    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>> {
        Box::pin(async { unreachable!("test only observes endpoints") })
    }
    fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("test only observes endpoints") })
    }
    fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { unreachable!("test only observes endpoints") })
    }
}

fn probe(store: &NetworkMembershipStore, endpoint: SocketAddrV4, mode: ObservationMode) -> Probe {
    Probe {
        endpoint,
        membership_path: store.path().to_owned(),
        mode,
        calls: AtomicUsize::new(0),
    }
}

fn assert_closed(mut stream: TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    match stream.read(&mut [0_u8; 1]) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                IoErrorKind::ConnectionReset
                    | IoErrorKind::ConnectionAborted
                    | IoErrorKind::BrokenPipe
            ) => {}
        other => panic!("untrusted candidate survived retirement: {other:?}"),
    }
}

#[test]
fn durable_poll_checks_idle_once_and_accepted_candidate_twice() {
    let (root, store) = member_store("stable");
    let (mut owner, endpoint) = listener();
    let backend = probe(&store, endpoint, ObservationMode::Stable);
    assert!(
        futures_lite::future::block_on(
            owner.poll_from_durable_network_operations(&store, &backend)
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);

    let mut client = TcpStream::connect(endpoint).unwrap();
    client.write_all(b"u").unwrap();
    let mut accepted = futures_lite::future::block_on(
        owner.poll_from_durable_network_operations(&store, &backend),
    )
    .unwrap()
    .unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
    let mut read = [0_u8; 1];
    accepted.read_exact(&mut read).unwrap();
    assert_eq!(&read, b"u");
    drop(accepted);
    drop(client);
    drop(owner);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn membership_revocation_during_post_accept_backend_observation_retires_listener() {
    let (root, store) = member_store("revoked");
    let (mut owner, endpoint) = listener();
    let backend = probe(
        &store,
        endpoint,
        ObservationMode::RevokeDuringSecondObservation,
    );
    let queued = TcpStream::connect(endpoint).unwrap();
    let error = futures_lite::future::block_on(
        owner.poll_from_durable_network_operations(&store, &backend),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidState);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    assert!(store.load().unwrap().is_empty());
    assert_closed(queued);
    assert!(TcpStream::connect(endpoint).is_err());
    // A retired listener never invokes another backend or revives on rollback.
    assert_eq!(
        futures_lite::future::block_on(
            owner.poll_from_durable_network_operations(&store, &backend)
        )
        .unwrap_err()
        .kind(),
        ErrorKind::InvalidState
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn post_accept_durable_network_replacement_and_endpoint_drift_fail_closed() {
    for (label, mode) in [
        (
            "replacement",
            ObservationMode::ReplaceNetworkDuringSecondObservation,
        ),
        (
            "endpoint-drift",
            ObservationMode::EndpointDriftDuringSecondObservation,
        ),
    ] {
        let (root, store) = member_store(label);
        let (mut owner, endpoint) = listener();
        let backend = probe(&store, endpoint, mode);
        let mut queued = TcpStream::connect(endpoint).unwrap();
        queued.write_all(b"u").unwrap();

        let error = futures_lite::future::block_on(
            owner.poll_from_durable_network_operations(&store, &backend),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
        assert_closed(queued);
        assert!(TcpStream::connect(endpoint).is_err());
        assert_eq!(
            futures_lite::future::block_on(
                owner.poll_from_durable_network_operations(&store, &backend)
            )
            .unwrap_err()
            .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);

        let registry = store.load().unwrap();

        if matches!(mode, ObservationMode::ReplaceNetworkDuringSecondObservation) {
            assert!(registry.network(&network(8)).is_some());
            assert!(registry.network(&network(7)).is_none());
        } else {
            assert!(registry.network(&network(7)).is_some());
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn already_revoked_durable_membership_retires_without_backend_query() {
    let (root, store) = member_store("already-revoked");
    store.save(&MembershipRegistry::default()).unwrap();
    let (mut owner, endpoint) = listener();
    // If this backend is invoked it returns an error; no query is justified
    // once durable local membership already disproves listener ownership.
    let backend = probe(&store, endpoint, ObservationMode::BackendFailure);
    let queued = TcpStream::connect(endpoint).unwrap();
    let error = futures_lite::future::block_on(
        owner.poll_from_durable_network_operations(&store, &backend),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidState);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_closed(queued);
    assert!(TcpStream::connect(endpoint).is_err());
    assert_eq!(
        futures_lite::future::block_on(
            owner.poll_from_durable_network_operations(&store, &backend)
        )
        .unwrap_err()
        .kind(),
        ErrorKind::InvalidState
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn endpoint_backend_failure_retires_queued_candidates_without_tls() {
    let (root, store) = member_store("backend");
    let (mut owner, endpoint) = listener();
    let backend = probe(&store, endpoint, ObservationMode::BackendFailure);
    let queued = TcpStream::connect(endpoint).unwrap();
    let error = futures_lite::future::block_on(
        owner.poll_from_durable_network_operations(&store, &backend),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidState);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_closed(queued);
    assert!(TcpStream::connect(endpoint).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corruption_during_post_accept_durable_observation_closes_candidate_without_repair() {
    let (root, store) = member_store("post-accept-corrupt");
    let (mut owner, endpoint) = listener();
    let backend = probe(&store, endpoint, ObservationMode::CorruptDuringSecondObservation);
    let queued = TcpStream::connect(endpoint).unwrap();

    let error = futures_lite::future::block_on(
        owner.poll_from_durable_network_operations(&store, &backend),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::PersistenceError);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    assert_eq!(fs::read(store.path()).unwrap(), b"BLUEROUTE_MEMBERSHIP_V999\n");
    assert_closed(queued);
    assert!(TcpStream::connect(endpoint).is_err());
    assert_eq!(
        futures_lite::future::block_on(
            owner.poll_from_durable_network_operations(&store, &backend)
        )
        .unwrap_err()
        .kind(),
        ErrorKind::InvalidState,
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_durable_membership_retires_queued_listener_without_backend_or_repair() {
    for (label, corrupt) in [
        ("future-schema", b"BLUEROUTE_MEMBERSHIP_V999\n".as_slice()),
        (
            "malformed-header",
            b"invalid membership header\n".as_slice(),
        ),
        ("invalid-utf8", b"\xff\xfe".as_slice()),
    ] {
        let (root, store) = member_store(label);
        fs::write(store.path(), corrupt).unwrap();
        let original = fs::read(store.path()).unwrap();
        let (mut owner, endpoint) = listener();
        // The backend must not be consulted when durable membership cannot
        // be authenticated. The queued unauthenticated socket must be closed.
        let backend = probe(&store, endpoint, ObservationMode::BackendFailure);
        let queued = TcpStream::connect(endpoint).unwrap();
        let error = futures_lite::future::block_on(
            owner.poll_from_durable_network_operations(&store, &backend),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PersistenceError, "{label}");
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0, "{label}");
        assert_closed(queued);
        assert!(TcpStream::connect(endpoint).is_err(), "{label}");
        assert_eq!(fs::read(store.path()).unwrap(), original, "{label}");
        assert_eq!(
            futures_lite::future::block_on(
                owner.poll_from_durable_network_operations(&store, &backend)
            )
            .unwrap_err()
            .kind(),
            ErrorKind::InvalidState,
            "{label}",
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0, "{label}");
        fs::remove_dir_all(root).unwrap();
    }
}

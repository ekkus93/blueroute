//! P7-003 concurrent first-start TLS identity persistence regression.
//!
//! Several daemon workers racing on an empty durable state directory must
//! converge to exactly one local certificate/key pair. This tests only local
//! storage. It does not approve a peer, prove remote TLS, or activate JoinNetwork.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};
use std::thread;

use blueroute_core::NodeId;
use blueroute_daemon::ControlTlsIdentityStore;

#[test]
fn simultaneous_initializations_converge_to_one_persisted_tls_identity() {
    let root = std::env::temp_dir().join(format!(
        "blueroute-concurrent-control-identity-{}-{:?}",
        std::process::id(),
        thread::current().id()
    ));
    fs::create_dir(&root).unwrap();
    let identity_dir = root.join("control-tls");
    let node = NodeId::from_bytes([0x41; 16]);
    let threads = 12;
    let gate = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let gate = Arc::clone(&gate);
            let identity_dir = identity_dir.clone();
            thread::spawn(move || {
                gate.wait();
                let identity = ControlTlsIdentityStore::new(identity_dir)
                    .load_or_create(node)
                    .unwrap();
                (
                    identity.node(),
                    identity.fingerprint(),
                    identity.certificate().as_ref().to_vec(),
                )
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    for entry in &results {
        assert_eq!(
            entry, &results[0],
            "one state directory must yield one TLS identity"
        );
    }
    let reopened = ControlTlsIdentityStore::new(&identity_dir)
        .load_or_create(node)
        .unwrap();
    assert_eq!(reopened.fingerprint(), results[0].1);
    assert_eq!(reopened.certificate().as_ref(), results[0].2.as_slice());

    // An alternate NodeId may not silently rotate or overwrite that key.
    let wrong_node = NodeId::from_bytes([0x42; 16]);
    assert!(
        ControlTlsIdentityStore::new(&identity_dir)
            .load_or_create(wrong_node)
            .is_err()
    );
    let unchanged = ControlTlsIdentityStore::new(&identity_dir)
        .load_or_create(node)
        .unwrap();
    assert_eq!(unchanged.fingerprint(), results[0].1);

    let mode = fs::metadata(identity_dir.join("control-tls-identity-v1"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        fs::metadata(&identity_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn conflicting_node_id_first_start_never_creates_two_local_tls_owners() {
    // Two independent daemon processes can accidentally target the same
    // state directory. Precisely one NodeId may claim the persisted key.
    for trial in 0..4 {
        let root = std::env::temp_dir().join(format!(
            "blueroute-conflicting-control-identity-{}-{:?}-{trial}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir(&root).unwrap();
        let identity_dir = root.join("control-tls");
        let gate = Arc::new(Barrier::new(2));
        let ids = [NodeId::from_bytes([0x51; 16]), NodeId::from_bytes([0x52; 16])];
        let handles: Vec<_> = ids
            .into_iter()
            .map(|node| {
                let path = identity_dir.clone();
                let gate = Arc::clone(&gate);
                thread::spawn(move || {
                    gate.wait();
                    (node, ControlTlsIdentityStore::new(path)
                        .load_or_create(node)
                        .map(|identity| identity.fingerprint()))
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        let winners: Vec<_> = results.iter().filter(|(_, result)| result.is_ok()).collect();
        assert_eq!(winners.len(), 1, "exactly one NodeId may own local TLS state");
        let (winner, fingerprint) = winners[0];
        let fingerprint = fingerprint.as_ref().unwrap();
        let loser = if *winner == ids[0] { ids[1] } else { ids[0] };
        assert!(ControlTlsIdentityStore::new(&identity_dir)
            .load_or_create(loser)
            .is_err());
        let reopened = ControlTlsIdentityStore::new(&identity_dir)
            .load_or_create(*winner)
            .unwrap();
        assert_eq!(&reopened.fingerprint(), fingerprint);
        fs::remove_dir_all(root).unwrap();
    }
}

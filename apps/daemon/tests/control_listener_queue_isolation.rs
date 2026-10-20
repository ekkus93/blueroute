//! P7-004 queued TCP candidates must not survive loss of NAP ownership.
//!
//! Localhost socket evidence only: no peer authentication, PAN hardware,
//! independent enrollment, or production daemon listener is exercised.

use std::io::{Read, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::time::Duration;

use blueroute_core::{ErrorKind, NetworkId};
use blueroute_daemon::ObservedControlListener;

fn listener_owner(byte: u8) -> (ObservedControlListener, SocketAddrV4) {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        panic!("loopback test listener must be IPv4");
    };
    let owner = ObservedControlListener::new(NetworkId::from_bytes([byte; 16]), address, socket)
        .unwrap();
    (owner, address)
}

#[test]
fn ownership_loss_discards_all_queued_candidates_without_affecting_other_listener() {
    let network = NetworkId::from_bytes([7; 16]);
    let (mut owner, address) = listener_owner(7);
    let (mut independent, independent_address) = listener_owner(8);

    // Multiple TCP handshakes can finish before a daemon observes NAP loss.
    // None may escape the retired listener merely because they were queued.
    let mut queued = (0..4)
        .map(|_| {
            let mut socket = TcpStream::connect(address).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            socket.write_all(b"untrusted").unwrap();
            socket
        })
        .collect::<Vec<_>>();

    let error = owner.accept_if_current(None, Some(address)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidState);

    // A later matching observation must not resurrect a retired listener.
    assert_eq!(
        owner
            .accept_if_current(Some(network), Some(address))
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidState
    );

    // Linux may signal queued connection teardown as EOF or reset, but must
    // never leave a live accepted socket or indefinitely pending read.
    for socket in &mut queued {
        match socket.read(&mut [0_u8; 1]) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                ) => {}
            other => panic!("queued connection survived listener retirement: {other:?}"),
        }
    }

    // Another logical network's independently owned listener stays usable.
    let mut remote = TcpStream::connect(independent_address).unwrap();
    remote.write_all(b"ok").unwrap();
    let mut accepted = independent
        .accept_if_current(
            Some(NetworkId::from_bytes([8; 16])),
            Some(independent_address),
        )
        .unwrap()
        .unwrap();
    let mut bytes = [0_u8; 2];
    accepted.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"ok");
}

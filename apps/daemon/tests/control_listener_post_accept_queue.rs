//! P7-004 post-accept ownership failure must discard every queued TCP candidate.
//!
//! Localhost-only negative evidence. These sockets are untrusted: this test
//! does not establish peer enrollment, TLS, PAN, or production JoinNetwork.

use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::time::Duration;

use blueroute_core::{CoreError, ErrorKind, NetworkId};
use blueroute_daemon::ObservedControlListener;

fn listener(byte: u8) -> (ObservedControlListener, SocketAddrV4) {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let SocketAddr::V4(endpoint) = socket.local_addr().unwrap() else {
        panic!("localhost listener must be IPv4");
    };
    (
        ObservedControlListener::new(NetworkId::from_bytes([byte; 16]), endpoint, socket).unwrap(),
        endpoint,
    )
}

fn assert_closed(mut socket: TcpStream) {
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    match socket.read(&mut [0_u8; 1]) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                IoErrorKind::ConnectionReset
                    | IoErrorKind::ConnectionAborted
                    | IoErrorKind::BrokenPipe
            ) => {}
        other => panic!("untrusted TCP candidate survived listener retirement: {other:?}"),
    }
}

#[test]
fn post_accept_ownership_failure_closes_accepted_and_queued_candidates() {
    for failure in 0..3 {
        let network = NetworkId::from_bytes([7; 16]);
        let (mut owner, endpoint) = listener(7);
        let (mut independent, other_endpoint) = listener(8);

        // The first candidate will be accepted before the second observation.
        // Remaining candidates must not survive the listener's retirement.
        let queued = (0..4)
            .map(|_| {
                let mut socket = TcpStream::connect(endpoint).unwrap();
                socket.write_all(b"untrusted").unwrap();
                socket
            })
            .collect::<Vec<_>>();

        let mut observations = 0;
        let error = owner
            .poll_with_fresh_observations(|| {
                observations += 1;
                if observations == 1 {
                    return Ok((Some(network), Some(endpoint)));
                }
                match failure {
                    0 => Ok((None, Some(endpoint))),
                    1 => Ok((
                        Some(network),
                        Some(SocketAddrV4::new(
                            *endpoint.ip(),
                            endpoint.port().wrapping_add(1),
                        )),
                    )),
                    _ => Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "fresh NAP backend observation failed",
                    )),
                }
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(observations, 2, "recheck must occur after TCP accept");

        for socket in queued {
            assert_closed(socket);
        }
        assert!(TcpStream::connect(endpoint).is_err());
        assert_eq!(
            owner
                .accept_if_current(Some(network), Some(endpoint))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState,
            "a matching observation must not revive the retired listener"
        );

        // Retiring one logical network must not disturb another listener.
        let mut client = TcpStream::connect(other_endpoint).unwrap();
        client.write_all(b"ok").unwrap();
        let mut accepted = independent
            .accept_if_current(Some(NetworkId::from_bytes([8; 16])), Some(other_endpoint))
            .unwrap()
            .unwrap();
        let mut bytes = [0_u8; 2];
        accepted.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ok");
    }
}

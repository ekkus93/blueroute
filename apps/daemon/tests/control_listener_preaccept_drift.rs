//! P7-004 pre-accept NAP ownership drift must discard queued TCP candidates.
//!
//! Loopback-only fault-injection evidence. No mutual TLS, independent
//! enrollment, Bluetooth PAN or production JoinNetwork is qualified.

use std::io::{ErrorKind as IoErrorKind, Read};
use std::net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::time::Duration;

use blueroute_core::{ErrorKind, NetworkId};
use blueroute_daemon::ObservedControlListener;

fn network(value: u8) -> NetworkId {
    NetworkId::from_bytes([value; 16])
}

#[test]
fn preaccept_ownership_mismatch_retires_all_queued_sockets_without_reobserving() {
    for scenario in 0..3 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let SocketAddr::V4(endpoint) = listener.local_addr().unwrap() else {
            panic!("loopback listener must be IPv4");
        };
        let mut owner = ObservedControlListener::new(network(7), endpoint, listener).unwrap();

        let queued = (0..3)
            .map(|_| {
                let socket = TcpStream::connect(endpoint).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
            })
            .collect::<Vec<_>>();

        let mut observations = 0;
        let error = owner
            .poll_with_fresh_observations(|| {
                observations += 1;
                let snapshot = match scenario {
                    0 => (None, Some(endpoint)),
                    1 => (Some(network(8)), Some(endpoint)),
                    _ => (
                        Some(network(7)),
                        Some(SocketAddrV4::new(*endpoint.ip(), endpoint.port().wrapping_add(1))),
                    ),
                };
                Ok(snapshot)
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(observations, 1, "pre-accept mismatch must not reobserve");
        assert!(TcpStream::connect(endpoint).is_err());

        for mut socket in queued {
            match socket.read(&mut [0_u8; 1]) {
                Ok(0) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        IoErrorKind::ConnectionReset
                            | IoErrorKind::ConnectionAborted
                            | IoErrorKind::BrokenPipe
                    ) => {}
                other => panic!("queued candidate survived ownership loss: {other:?}"),
            }
        }

        let mut after_retirement_observations = 0;
        assert_eq!(
            owner
                .poll_with_fresh_observations(|| {
                    after_retirement_observations += 1;
                    Ok((Some(network(7)), Some(endpoint)))
                })
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(after_retirement_observations, 0);
    }
}

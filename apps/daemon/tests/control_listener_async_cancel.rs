//! P7-004 cancellation boundaries for awaited, fresh NAP ownership checks.
//!
//! Loopback-only software regression. Cancellation cannot release an
//! unauthenticated TCP candidate while a backend observation is pending.
//! This does not prove TLS enrollment, PAN ownership, or production joining.

use std::future::Future;
use std::io::{ErrorKind as IoErrorKind, Read};
use std::net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use blueroute_core::{ErrorKind, NetworkId};
use blueroute_daemon::ObservedControlListener;

fn listener() -> (ObservedControlListener, SocketAddrV4) {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let SocketAddr::V4(endpoint) = socket.local_addr().unwrap() else {
        panic!("loopback listener must be IPv4");
    };
    (
        ObservedControlListener::new(NetworkId::from_bytes([7; 16]), endpoint, socket).unwrap(),
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
        other => panic!("cancelled or retired listener retained a TCP candidate: {other:?}"),
    }
}

#[test]
fn cancellation_during_post_accept_observation_drops_candidate_and_rechecks_next_poll() {
    let (mut owner, endpoint) = listener();
    let accepted_candidate = TcpStream::connect(endpoint).unwrap();
    let mut observations = 0;
    {
        let mut poll = Box::pin(owner.poll_with_async_observations(|| {
            observations += 1;
            let observation = observations;
            async move {
                if observation == 1 {
                    Ok((Some(NetworkId::from_bytes([7; 16])), Some(endpoint)))
                } else {
                    std::future::pending().await
                }
            }
        }));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(poll.as_mut().poll(&mut context), Poll::Pending));
        // Drop while the second independent observation is still pending.
        // The accepted, unauthenticated socket must be owned by that future.
    }
    assert_eq!(observations, 2);
    assert_closed(accepted_candidate);

    // Cancellation alone does not establish that ownership was lost; the
    // next poll must obtain fresh evidence and retire on a changed network.
    let queued = TcpStream::connect(endpoint).unwrap();
    let mut fresh = 0;
    assert_eq!(
        owner
            .poll_with_fresh_observations(|| {
                fresh += 1;
                Ok((None, Some(endpoint)))
            })
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidState
    );
    assert_eq!(fresh, 1);
    assert_closed(queued);
    assert!(TcpStream::connect(endpoint).is_err());
}

#[test]
fn cancellation_during_pre_accept_observation_cannot_bypass_next_fresh_gate() {
    let (mut owner, endpoint) = listener();
    let queued = TcpStream::connect(endpoint).unwrap();
    let mut observations = 0;
    {
        let mut poll = Box::pin(owner.poll_with_async_observations(|| {
            observations += 1;
            async {
                std::future::pending::<
                    Result<(Option<NetworkId>, Option<SocketAddrV4>), blueroute_core::CoreError>,
                >()
                .await
            }
        }));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(poll.as_mut().poll(&mut context), Poll::Pending));
    }
    assert_eq!(observations, 1);

    // A cancelled first observation never authorizes a queued candidate.
    // Even a later valid snapshot cannot resurrect a retired listener.
    assert_eq!(
        owner
            .poll_with_fresh_observations(|| Ok((
                Some(NetworkId::from_bytes([8; 16])),
                Some(endpoint)
            )))
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidState
    );
    assert_closed(queued);
    assert!(TcpStream::connect(endpoint).is_err());
}

//! Source-bound, deadline-limited TCP dial for Linux PANU control sessions.
//!
//! Bind the owned IPv4 address *before* sending a SYN; checking the source
//! only after a regular TcpStream::connect_timeout could send traffic over an
//! unrelated interface. This does not establish Bluetooth interface ownership:
//! callers must still observe the PAN link and verify the authenticated peer.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Instant;

use rustix::event::{PollFd, PollFlags, poll};
use rustix::io::Errno;
use rustix::net::sockopt::socket_error;
use rustix::net::{AddressFamily, SocketFlags, SocketType, bind, connect, socket_with};
use rustix::time::Timespec;

/// Open a TCP stream from the exact locally owned IPv4 address by a monotonic
/// deadline. A missing source address fails before any outbound TCP connect.
///
/// The returned stream is blocking (suitable for rustls); all intermediate
/// descriptors are RAII-owned and closed on failure or timeout.
pub fn connect_tcp_from_ipv4(
    source: Ipv4Addr,
    destination: SocketAddrV4,
    deadline: Instant,
) -> io::Result<TcpStream> {
    fn timeout() -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "source-bound TCP dial deadline expired",
        )
    }

    if Instant::now() >= deadline {
        return Err(timeout());
    }
    let fd = socket_with(
        AddressFamily::INET,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .map_err(io::Error::from)?;
    bind(&fd, &SocketAddrV4::new(source, 0)).map_err(io::Error::from)?;
    if Instant::now() >= deadline {
        return Err(timeout());
    }
    match connect(&fd, &destination) {
        Ok(()) => {}
        Err(Errno::INPROGRESS) => loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(timeout());
            }
            let wait = Timespec {
                tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
                tv_nsec: i64::from(remaining.subsec_nanos()),
            };
            let mut ready = [PollFd::new(&fd, PollFlags::OUT)];
            match poll(&mut ready, Some(&wait)) {
                Ok(0) => return Err(timeout()),
                Ok(_) => match socket_error(&fd).map_err(io::Error::from)? {
                    Ok(()) => break,
                    Err(error) => return Err(io::Error::from(error)),
                },
                Err(Errno::INTR) => continue,
                Err(error) => return Err(io::Error::from(error)),
            }
        },
        Err(error) => return Err(io::Error::from(error)),
    }
    if Instant::now() >= deadline {
        return Err(timeout());
    }
    let stream = TcpStream::from(fd);
    stream.set_nonblocking(false)?;
    if stream.local_addr()?.ip() != IpAddr::V4(source) {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "TCP socket did not retain its explicitly bound source address",
        ));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn loopback_dial_uses_explicit_source_and_returns_blocking_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let destination = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(address) => address,
            _ => unreachable!(),
        };
        let stream = connect_tcp_from_ipv4(
            Ipv4Addr::LOCALHOST,
            destination,
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap();
        let (accepted, remote) = listener.accept().unwrap();
        assert_eq!(
            stream.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(remote.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        drop(accepted);
    }

    #[test]
    fn unassigned_source_address_fails_before_connect() {
        let result = connect_tcp_from_ipv4(
            Ipv4Addr::new(192, 0, 2, 123),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9),
            Instant::now() + Duration::from_secs(2),
        );
        assert!(result.is_err(), "unassigned source must not dial");
    }

    #[test]
    fn expired_deadline_refuses_even_a_loopback_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let destination = match listener.local_addr().unwrap() {
            std::net::SocketAddr::V4(address) => address,
            _ => unreachable!(),
        };
        let error = connect_tcp_from_ipv4(
            Ipv4Addr::LOCALHOST,
            destination,
            Instant::now() - Duration::from_millis(1),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}

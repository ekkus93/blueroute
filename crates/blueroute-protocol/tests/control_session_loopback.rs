//! P7-004 deterministic loopback integration across Hello/HelloAck and
//! post-hello Heartbeat/Goodbye on two real TCP sockets.
//!
//! The exporter, challenges and independently verified connection preconditions
//! are synthetic. This is NOT mutual TLS, private-key proof, Bluetooth PAN
//! transport, enrollment, or production JoinNetwork acceptance.

use std::io::Read;
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use blueroute_core::{NetworkId, NodeCapabilities, NodeId};
use blueroute_protocol::control::{ControlMessage, HelloFacts};
use blueroute_protocol::handshake::{HelloExchangeRequest, exchange_hello_claims_with_deadline};
use blueroute_protocol::session_stream::{SessionControlStream, SessionReadIoError};
use blueroute_protocol::session_writer::SessionControlWriter;

fn network() -> NetworkId {
    NetworkId::from_bytes([1; 16])
}

fn alice() -> NodeId {
    NodeId::from_bytes([2; 16])
}

fn bob() -> NodeId {
    NodeId::from_bytes([3; 16])
}

fn facts() -> HelloFacts {
    HelloFacts {
        software_version: "0.1.0".to_owned(),
        capabilities: NodeCapabilities::default(),
        segment: None,
        direct_neighbors: Vec::new(),
    }
}

fn hello(stream: &mut TcpStream, local: NodeId, remote: NodeId, challenge: [u8; 16]) {
    let received = exchange_hello_claims_with_deadline(
        stream,
        HelloExchangeRequest {
            network: network(),
            local,
            remote,
            exported_session_id: [4; 16],
            local_challenge: challenge,
            local_facts: facts(),
            deadline: Instant::now() + Duration::from_secs(5),
        },
    )
    .unwrap();
    assert_eq!(received, facts());
}

fn lifecycle(mut stream: TcpStream, local: NodeId, remote: NodeId, challenge: [u8; 16]) {
    stream.set_nodelay(true).unwrap();
    hello(&mut stream, local, remote, challenge);
    // The fixture does not prove TLS authentication: the constructor is used
    // to exercise only post-hello session framing and replay checks.
    let mut outbound =
        SessionControlWriter::after_verified_hello(network(), local, remote, [4; 16]).unwrap();
    let mut inbound =
        SessionControlStream::after_verified_hello(network(), remote, [4; 16]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    outbound
        .write_heartbeat_with_deadline(&mut stream, deadline)
        .unwrap();
    let received = inbound
        .read_one_with_deadline(&mut stream, deadline)
        .unwrap();
    assert_eq!(received.sender, remote);
    assert_eq!(received.sequence, 3);
    assert!(matches!(received.message, ControlMessage::Heartbeat));

    outbound
        .write_goodbye_with_deadline(&mut stream, deadline)
        .unwrap();
    let received = inbound
        .read_one_with_deadline(&mut stream, deadline)
        .unwrap();
    assert_eq!(received.sequence, 4);
    assert!(matches!(received.message, ControlMessage::Goodbye));
    assert!(outbound.is_closed());
    assert!(inbound.is_closed());

    // A TLS adapter must likewise close after Goodbye; test real TCP
    // half-close ordering rather than assuming goodbye implies socket EOF.
    stream.shutdown(Shutdown::Write).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut extra = [0_u8; 1];
    assert_eq!(stream.read(&mut extra).unwrap(), 0);
    assert_eq!(inbound.finish(), Ok(()));
}

#[test]
fn loopback_two_peer_hello_heartbeat_goodbye_and_tcp_half_close() {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        lifecycle(socket, bob(), alice(), [6; 16]);
    });
    let client = TcpStream::connect(addr).unwrap();
    lifecycle(client, alice(), bob(), [5; 16]);
    server.join().unwrap();
}

#[test]
fn loopback_unannounced_loss_poisoned_after_valid_hello() {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        hello(&mut socket, bob(), alice(), [7; 16]);
        let mut inbound =
            SessionControlStream::after_verified_hello(network(), alice(), [4; 16]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(matches!(
            inbound
                .read_one_with_deadline(&mut socket, deadline)
                .unwrap()
                .message,
            ControlMessage::Heartbeat
        ));
        // The remote terminated TCP without sending authenticated Goodbye.
        assert!(matches!(
            inbound.read_one_with_deadline(&mut socket, deadline),
            Err(SessionReadIoError::Transport(_))
        ));
        assert!(matches!(
            inbound.read_one_with_deadline(&mut socket, deadline),
            Err(SessionReadIoError::Scope(
                blueroute_protocol::session_stream::SessionStreamError::Poisoned
            ))
        ));
    });
    let mut client = TcpStream::connect(addr).unwrap();
    hello(&mut client, alice(), bob(), [8; 16]);
    let mut outbound =
        SessionControlWriter::after_verified_hello(network(), alice(), bob(), [4; 16]).unwrap();
    outbound
        .write_heartbeat_with_deadline(&mut client, Instant::now() + Duration::from_secs(5))
        .unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    server.join().unwrap();
}

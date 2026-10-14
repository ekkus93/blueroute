//! P7-004 simulated control-session transcript over local TCP.
//! This DOES NOT authenticate peers: the production transport must provide
//! verified mutual TLS 1.3, pinned enrollment, NetworkId/NodeId authorization
//! and a session identifier exported from that exact verified connection.

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use blueroute_core::{NetworkId, NodeCapabilities, NodeId};
use blueroute_protocol::control::HelloFacts;
use blueroute_protocol::handshake::{
    HelloExchangeError, HelloExchangeRequest, exchange_hello_claims_with_deadline,
};

fn claims() -> HelloFacts {
    HelloFacts {
        software_version: "0.1.0".to_owned(),
        capabilities: NodeCapabilities::default(),
        segment: None,
        direct_neighbors: vec![],
    }
}

fn request(local: u8, remote: u8, challenge: u8) -> HelloExchangeRequest {
    HelloExchangeRequest {
        network: NetworkId::from_bytes([1; 16]),
        local: NodeId::from_bytes([local; 16]),
        remote: NodeId::from_bytes([remote; 16]),
        exported_session_id: [9; 16],
        local_challenge: [challenge; 16],
        local_facts: claims(),
        deadline: Instant::now() + Duration::from_secs(3),
    }
}

type ExchangeResult = Result<HelloFacts, HelloExchangeError>;

/// All traffic stays on 127.0.0.1 and uses self-supplied test values; no
/// actual TLS, PIN, BlueZ or PAN connection exists in this test.
fn run_pair(
    host: HelloExchangeRequest,
    client: HelloExchangeRequest,
) -> (ExchangeResult, ExchangeResult) {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        exchange_hello_claims_with_deadline(&mut socket, host)
    });

    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(3)).unwrap();
    let client_result = exchange_hello_claims_with_deadline(&mut socket, client);
    drop(socket);
    (server.join().unwrap(), client_result)
}

#[test]
fn symmetric_hello_and_ack_complete_over_two_simulated_peers() {
    let (server, client) = run_pair(request(4, 5, 10), request(5, 4, 11));
    assert_eq!(server.unwrap(), claims());
    assert_eq!(client.unwrap(), claims());
}

#[test]
fn mismatched_network_node_or_exporter_prevents_either_side_from_joining() {
    let mut wrong_network = request(5, 4, 11);
    wrong_network.network = NetworkId::from_bytes([2; 16]);
    let (server, client) = run_pair(request(4, 5, 10), wrong_network);
    assert!(server.is_err());
    assert!(client.is_err());

    let mut wrong_node = request(5, 4, 11);
    wrong_node.local = NodeId::from_bytes([6; 16]);
    let (server, client) = run_pair(request(4, 5, 10), wrong_node);
    assert!(server.is_err());
    assert!(client.is_err());

    let mut wrong_exporter = request(5, 4, 11);
    wrong_exporter.exported_session_id = [8; 16];
    let (server, client) = run_pair(request(4, 5, 10), wrong_exporter);
    assert!(server.is_err());
    assert!(client.is_err());
}

#[test]
fn reflected_challenge_cannot_complete_either_exchange() {
    let (server, client) = run_pair(request(4, 5, 10), request(5, 4, 10));
    assert!(server.is_err());
    assert!(client.is_err());
}

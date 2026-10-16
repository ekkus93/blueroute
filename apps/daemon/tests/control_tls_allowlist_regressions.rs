//! Public-API regressions for ambiguous inbound TLS certificate allowlists.
//! These checks run on loopback only and are not Bluetooth/PAN acceptance.

use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use blueroute_core::{NetworkId, NodeId};
use blueroute_daemon::{
    ControlTlsError, LocalControlTlsIdentity, PinnedControlTlsPeer,
    exchange_control_tls_certificate,
};
use blueroute_protocol::peer_pin::SpkiSha256;

fn node(value: u8) -> NodeId {
    NodeId::from_bytes([value; 16])
}

fn network(value: u8) -> NetworkId {
    NetworkId::from_bytes([value; 16])
}

fn pin(network: NetworkId, node: NodeId, value: u8) -> PinnedControlTlsPeer {
    let fingerprint = SpkiSha256::from_verified_certificate_digest([value; 32]).unwrap();
    PinnedControlTlsPeer::new(network, node, fingerprint).unwrap()
}

fn reject_without_network_io(
    local: &LocalControlTlsIdentity,
    network: NetworkId,
    allowed: &[PinnedControlTlsPeer],
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut socket, _) = listener.accept().unwrap();
    let result = exchange_control_tls_certificate(
        &mut socket,
        local,
        network,
        allowed,
        Instant::now() + Duration::from_secs(1),
    );
    assert!(matches!(result, Err(ControlTlsError::InvalidIdentity)));
}

#[test]
fn duplicate_node_with_different_fingerprints_fails_closed() {
    let local = LocalControlTlsIdentity::generate(node(1)).unwrap();
    reject_without_network_io(
        &local,
        network(9),
        &[pin(network(9), node(2), 4), pin(network(9), node(2), 5)],
    );
}

#[test]
fn duplicate_fingerprint_under_different_nodes_fails_closed() {
    let local = LocalControlTlsIdentity::generate(node(1)).unwrap();
    reject_without_network_io(
        &local,
        network(9),
        &[pin(network(9), node(2), 4), pin(network(9), node(3), 4)],
    );
}

#[test]
fn cross_network_pin_and_self_identity_fail_closed() {
    let local = LocalControlTlsIdentity::generate(node(1)).unwrap();
    reject_without_network_io(&local, network(9), &[pin(network(8), node(2), 4)]);
    reject_without_network_io(&local, network(9), &[pin(network(9), node(1), 4)]);
}

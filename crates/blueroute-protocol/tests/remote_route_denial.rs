//! P14-003 fail-closed remote routing/control boundary, before TLS integration.
//!
//! The current P7 protocol deliberately has NO route, topology mutation,
//! remote join or gateway command. Only an independently authenticated,
//! pinned and authorized future control transport may introduce one. These
//! negative tests do not implement or authenticate any remote route changes.

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::{
    CONTROL_VERSION, ControlEnvelope, ControlMessage, decode_control_frame, encode_control_frame,
};
use blueroute_protocol::session::{ConnectionDeadline, ConnectionStage};
use blueroute_protocol::session_activity::{AuthenticatedInboundActivity, InboundActivityError};

fn expected_network() -> NetworkId {
    NetworkId::from_bytes([61; 16])
}

fn expected_peer() -> NodeId {
    NodeId::from_bytes([62; 16])
}

fn valid_heartbeat() -> Vec<u8> {
    encode_control_frame(&ControlEnvelope {
        version: CONTROL_VERSION,
        network: expected_network(),
        sender: expected_peer(),
        session_id: [63; 16],
        sequence: 3,
        message: ControlMessage::Heartbeat,
    })
    .unwrap()
}

fn after_simulated_hello() -> AuthenticatedInboundActivity {
    let mut deadline = ConnectionDeadline::new(0);
    deadline.advance(ConnectionStage::TlsHandshake, 1).unwrap();
    deadline.advance(ConnectionStage::Hello, 2).unwrap();
    deadline.advance(ConnectionStage::Active, 3).unwrap();
    AuthenticatedInboundActivity::after_verified_hello(
        expected_network(),
        expected_peer(),
        [63; 16],
        deadline,
    )
    .unwrap()
}

fn forged_control_message(name: &str) -> Vec<u8> {
    let valid = valid_heartbeat();
    let mut payload: serde_json::Value = serde_json::from_slice(&valid[4..]).unwrap();
    payload["message"] = serde_json::json!({
        "type": name,
        "data": {
            "next_hop": "0.0.0.0",
            "metric": 0,
            "default_route": true,
            "authorize_join": true
        }
    });
    let body = serde_json::to_vec(&payload).unwrap();
    let size = u32::try_from(body.len()).unwrap();
    let mut framed = size.to_be_bytes().to_vec();
    framed.extend(body);
    framed
}

#[test]
fn future_route_membership_and_gateway_names_are_not_accepted_as_current_control() {
    for forbidden in [
        "route_update",
        "topology_update",
        "network_member_add",
        "approve_peer",
        "join_network",
        "set_default_route",
        "enable_internet_gateway",
        "route_delete",
    ] {
        let attack = forged_control_message(forbidden);
        assert!(decode_control_frame(&attack).is_err(), "{forbidden}");
        let mut session = after_simulated_hello();
        assert!(matches!(
            session.feed_verified_plaintext(&attack, 100),
            Err(InboundActivityError::Scope(_))
        ));
        assert!(session.is_terminal());
        assert_eq!(session.expires_ms(), 30_003);
        assert_eq!(
            session.feed_verified_plaintext(&valid_heartbeat(), 101),
            Err(InboundActivityError::Terminal)
        );
    }
}

#[test]
fn one_valid_heartbeat_before_illegal_route_command_is_atomic_failure() {
    let good = valid_heartbeat();
    let mut combined = good.clone();
    combined.extend(forged_control_message("route_update"));
    let mut session = after_simulated_hello();
    assert!(matches!(
        session.feed_verified_plaintext(&combined, 100),
        Err(InboundActivityError::Scope(_))
    ));
    assert_eq!(session.expires_ms(), 30_003);
    assert!(session.is_terminal());
    assert_eq!(
        session.feed_verified_plaintext(&good, 101),
        Err(InboundActivityError::Terminal)
    );
}

#[test]
fn route_like_extensions_on_current_heartbeat_are_not_silent_authority() {
    let original = valid_heartbeat();
    let mut payload: serde_json::Value = serde_json::from_slice(&original[4..]).unwrap();
    payload["allow_default_route"] = serde_json::json!(true);
    let bytes = serde_json::to_vec(&payload).unwrap();
    let mut attack = u32::try_from(bytes.len()).unwrap().to_be_bytes().to_vec();
    attack.extend(bytes);
    assert!(decode_control_frame(&attack).is_err());
    let mut session = after_simulated_hello();
    assert!(session.feed_verified_plaintext(&attack, 100).is_err());
    assert!(session.is_terminal());
}

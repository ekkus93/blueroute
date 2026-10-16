//! A stale PAN loss must not retire an authenticated replacement.
use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::session::{ControlSessionRegistry, PeerSessionKey, SessionPhase};

#[test]
fn stale_loss_and_retirement_preserve_new_authenticated_generation() {
    let key = PeerSessionKey {
        network: NetworkId::from_bytes([1; 16]),
        peer: NodeId::from_bytes([2; 16]),
    };
    let mut registry = ControlSessionRegistry::new();
    registry.activate_after_authentication(key, [3; 16]).unwrap();
    assert!(registry.mark_lost(key, [3; 16]));
    assert!(registry.mark_reconnecting(key, [3; 16]));
    registry.activate_after_authentication(key, [4; 16]).unwrap();
    assert!(!registry.mark_lost(key, [3; 16]));
    assert!(!registry.remove_if_current(key, [3; 16]));
    let active = registry.get(key).unwrap();
    assert_eq!(active.session_id, [4; 16]);
    assert_eq!(active.phase, SessionPhase::Active);
}

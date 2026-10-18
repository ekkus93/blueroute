//! Diagnostic API privacy and forward-compatibility regressions.
//!
//! The typed diagnostics contract intentionally has no field for raw API
//! error details, credentials or private-key material. Unknown fields supplied
//! by a newer or malicious producer must not be carried into re-serialized
//! snapshots by an older consumer.

use blueroute_core::{HealthLevel, NodeCapabilities};
use blueroute_protocol::{ApiVersion, DiagnosticSnapshot};
use serde_json::{Value, json};

fn snapshot() -> DiagnosticSnapshot {
    DiagnosticSnapshot {
        api_version: ApiVersion::CURRENT,
        health: HealthLevel::Degraded,
        current_network: None,
        visible_nodes: 0,
        capabilities: NodeCapabilities::default(),
        local_control_tls_spki_sha256: None,
        system: None,
        observed_peers: None,
        interfaces: None,
        routes: None,
        topology: None,
        recent_errors: Vec::new(),
    }
}

#[test]
fn diagnostics_drop_unrecognized_sensitive_fields_on_typed_round_trip() {
    const SENTINEL: &str = "PRIVATE_MATERIAL_MUST_NOT_APPEAR";
    let mut wire = serde_json::to_value(snapshot()).unwrap();
    wire["private_key"] = json!(SENTINEL);
    wire["credentials"] = json!({"password": SENTINEL});
    wire["recent_errors"] = json!([{
        "timestamp_unix_ms": 1234,
        "operation": "JoinNetwork",
        "failure_class": "denied",
        "raw_error_message": SENTINEL,
        "authorization_token": SENTINEL
    }]);

    let typed: DiagnosticSnapshot = serde_json::from_value(wire).unwrap();
    assert_eq!(typed.recent_errors.len(), 1);
    assert_eq!(typed.recent_errors[0].operation, "JoinNetwork");
    assert_eq!(typed.recent_errors[0].failure_class, "denied");

    let normalized = serde_json::to_string(&typed).unwrap();
    assert!(!normalized.contains(SENTINEL));
    assert!(!normalized.contains("private_key"));
    assert!(!normalized.contains("credentials"));
    assert!(!normalized.contains("raw_error_message"));
    assert!(!normalized.contains("authorization_token"));
}

#[test]
fn diagnostics_do_not_invent_observations_when_fields_are_absent() {
    let mut wire = serde_json::to_value(snapshot()).unwrap();
    for field in [
        "system",
        "observed_peers",
        "interfaces",
        "routes",
        "topology",
    ] {
        wire.as_object_mut().unwrap().remove(field);
    }
    let typed: DiagnosticSnapshot = serde_json::from_value(wire).unwrap();
    assert!(typed.system.is_none());
    assert!(typed.observed_peers.is_none());
    assert!(typed.interfaces.is_none());
    assert!(typed.routes.is_none());
    assert!(typed.topology.is_none());

    let encoded: Value = serde_json::to_value(typed).unwrap();
    assert!(encoded["interfaces"].is_null());
    assert!(encoded["routes"].is_null());
}

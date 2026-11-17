//! Reading and independently verifying the local control-TLS key without enrolling anything.

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::peer_pin::SpkiSha256;
use blueroute_protocol::{DaemonStatus, DiagnosticSnapshot};

/// The daemon's public identity is read through two separate API calls.
/// Re-read status after diagnostics so an intervening daemon restart, node
/// switch or network transition cannot silently produce a mixed tuple.
/// This is still a best-effort local snapshot, not a signed peer transcript.
pub(super) fn require_stable_local_identity(
    before: &DaemonStatus,
    diagnostics: &DiagnosticSnapshot,
    after: &DaemonStatus,
) -> Result<(), &'static str> {
    if before.api_version != diagnostics.api_version
        || before.api_version != after.api_version
        || before.local_node != after.local_node
        || before.current_network != diagnostics.current_network
        || before.current_network != after.current_network
    {
        Err("daemon identity changed during local-key inspection")
    } else {
        Ok(())
    }
}

/// Public local identity for out-of-band comparison. It is neither a trust
/// decision nor evidence of a remote certificate's private-key possession.
pub(super) fn format_local_key(
    local_node: Option<NodeId>,
    network: Option<NetworkId>,
    fingerprint: Option<&str>,
    json: bool,
) -> Result<String, &'static str> {
    let node = local_node
        .filter(|node| *node != NodeId::from_bytes([0; 16]))
        .ok_or("daemon has no valid local NodeId")?;
    if network == Some(NetworkId::from_bytes([0; 16])) {
        return Err("daemon supplied an invalid zero NetworkId");
    }
    let fingerprint = SpkiSha256::parse_canonical_hex(
        fingerprint.ok_or("daemon has no local TLS public-key fingerprint")?,
    )
    .map_err(|_| "daemon supplied an invalid TLS public-key fingerprint")?
    .to_canonical_hex();
    if json {
        Ok(serde_json::json!({
            "local_node": node,
            "network_id": network,
            "spki_sha256": fingerprint,
        })
        .to_string())
    } else {
        let network = network.map_or_else(
            || "none (not currently a network member)".to_owned(),
            |network| network.to_string(),
        );
        Ok(format!(
            "Local NetworkId: {network}\nLocal NodeId: {node}\nLocal control TLS SPKI SHA-256: {fingerprint}\nCompare all three values through an independent channel. This does not enroll or authorize a peer."
        ))
    }
}

/// On the *candidate peer's own machine*, compare a tuple obtained through
/// an independent operator channel with the locally running daemon's values.
/// This never consumes a remote advertisement, never writes pins, and cannot
/// replace TLS proof of possession or a PolicyKit-protected enrollment.
pub(super) fn verify_local_key(
    status: &DaemonStatus,
    snapshot: &DiagnosticSnapshot,
    network: Option<NetworkId>,
    node: NodeId,
    expected_fingerprint: &str,
    json: bool,
) -> Result<String, &'static str> {
    // A prospective PANU peer may not yet have a durable network. In that
    // bootstrap case compare only its local NodeId and SPKI, explicitly
    // withholding any claim that a NetworkId was checked. A supplied network
    // is checked against BOTH daemon observations, never an advertisement.
    if node == NodeId::from_bytes([0; 16])
        || status.local_node != Some(node)
        || status.api_version != snapshot.api_version
        || status.current_network == Some(NetworkId::from_bytes([0; 16]))
        || status.current_network != snapshot.current_network
        || network.is_some_and(|expected| {
            expected == NetworkId::from_bytes([0; 16]) || status.current_network != Some(expected)
        })
    {
        return Err("independently supplied identity tuple does not match this daemon");
    }
    let expected = SpkiSha256::parse_canonical_hex(expected_fingerprint)
        .map_err(|_| "invalid independently supplied TLS fingerprint")?
        .to_canonical_hex();
    let observed = SpkiSha256::parse_canonical_hex(
        snapshot
            .local_control_tls_spki_sha256
            .as_deref()
            .ok_or("daemon has no local TLS public-key fingerprint")?,
    )
    .map_err(|_| "daemon supplied an invalid TLS public-key fingerprint")?
    .to_canonical_hex();
    if expected != observed {
        return Err("independently supplied identity tuple does not match this daemon");
    }
    if json {
        Ok(serde_json::json!({
            "matches": true,
            "network_id": network,
            "network_verified": network.is_some(),
            "local_node": node,
            "spki_sha256": observed,
            "enrollment_performed": false,
        })
        .to_string())
    } else {
        let network_result = if network.is_some() {
            "The supplied NetworkId matches this daemon's current network."
        } else {
            "NetworkId was not checked (a prospective joiner may not yet be a member)."
        };
        Ok(format!(
            "The supplied NodeId and SPKI SHA-256 match this daemon's local values.\n{network_result}\nThis read-only comparison does not enroll or authorize a peer; authenticated TLS proof and an explicit enrollment decision are still required."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use blueroute_core::{HealthLevel, NetworkId, NodeCapabilities, NodeId};

    use blueroute_protocol::{ApiVersion, DaemonStatus, DiagnosticSnapshot};

    #[test]
    fn local_key_display_requires_both_valid_daemon_values_and_never_enrolls() {
        let node = NodeId::from_bytes([7; 16]);
        let digest = "ab".repeat(32);
        let network = NetworkId::from_bytes([9; 16]);
        let human = format_local_key(Some(node), Some(network), Some(&digest), false).unwrap();
        assert!(human.contains(&node.to_string()));
        assert!(human.contains(&digest));
        assert!(human.contains("does not enroll or authorize"));
        assert!(human.contains(&network.to_string()));
        let encoded = format_local_key(Some(node), Some(network), Some(&digest), true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(parsed["local_node"], node.to_string());
        assert_eq!(parsed["network_id"], network.to_string());
        assert_eq!(parsed["spki_sha256"], digest);
        assert!(format_local_key(None, Some(network), Some(&digest), false).is_err());
        assert!(
            format_local_key(
                Some(NodeId::from_bytes([0; 16])),
                Some(network),
                Some(&digest),
                false,
            )
            .is_err()
        );
        assert!(
            format_local_key(
                Some(node),
                Some(NetworkId::from_bytes([0; 16])),
                Some(&digest),
                true,
            )
            .is_err(),
            "a malformed daemon network must not be displayed as an approved identity"
        );
        assert!(format_local_key(Some(node), Some(network), None, true).is_err());
        assert!(format_local_key(Some(node), Some(network), Some(&"00".repeat(32)), true).is_err());
        assert!(format_local_key(Some(node), Some(network), Some(&"AB".repeat(32)), true).is_err());
        assert!(format_local_key(Some(node), Some(network), Some("invalid"), false).is_err());
        let no_network = format_local_key(Some(node), None, Some(&digest), true).unwrap();
        assert!(no_network.contains("\"network_id\":null"));
    }
    #[test]
    fn independent_local_key_comparison_rejects_network_node_and_fingerprint_drift() {
        let network = NetworkId::from_bytes([9; 16]);
        let node = NodeId::from_bytes([7; 16]);
        let digest = "ab".repeat(32);
        let status = DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(node),
            current_network: Some(network),
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        };
        let snapshot = DiagnosticSnapshot {
            api_version: ApiVersion::CURRENT,
            health: HealthLevel::Healthy,
            current_network: Some(network),
            visible_nodes: 1,
            capabilities: NodeCapabilities::default(),
            local_control_tls_spki_sha256: Some(digest.clone()),
            system: None,
            observed_peers: None,
            interfaces: None,
            routes: None,
            topology: None,
            authenticated_control_peers: None,
            recent_errors: Vec::new(),
        };
        let json =
            verify_local_key(&status, &snapshot, Some(network), node, &digest, true).unwrap();
        let result: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(result["matches"], true);
        assert_eq!(result["enrollment_performed"], false);
        assert_eq!(result["network_verified"], true);
        assert_eq!(result["network_id"], network.to_string());
        assert_eq!(result["local_node"], node.to_string());
        assert_eq!(result["spki_sha256"], digest);
        let human =
            verify_local_key(&status, &snapshot, Some(network), node, &digest, false).unwrap();
        assert!(human.contains("does not enroll or authorize"));

        let wrong_network = NetworkId::from_bytes([8; 16]);
        let wrong_node = NodeId::from_bytes([6; 16]);
        assert!(require_stable_local_identity(&status, &snapshot, &status).is_ok());
        let mut late_status = status.clone();
        late_status.current_network = Some(wrong_network);
        assert!(require_stable_local_identity(&status, &snapshot, &late_status).is_err());
        late_status = status.clone();
        late_status.local_node = Some(wrong_node);
        assert!(require_stable_local_identity(&status, &snapshot, &late_status).is_err());
        late_status = status.clone();
        late_status.api_version = ApiVersion::new(1, 0);
        assert!(require_stable_local_identity(&status, &snapshot, &late_status).is_err());
        let mut mixed_diagnostics = snapshot.clone();
        mixed_diagnostics.api_version = ApiVersion::new(1, 0);
        assert!(require_stable_local_identity(&status, &mixed_diagnostics, &status).is_err());
        assert!(
            verify_local_key(&status, &snapshot, Some(wrong_network), node, &digest, true).is_err()
        );
        assert!(
            verify_local_key(&status, &snapshot, Some(network), wrong_node, &digest, true).is_err()
        );
        assert!(
            verify_local_key(
                &status,
                &snapshot,
                Some(network),
                node,
                &"cd".repeat(32),
                true
            )
            .is_err()
        );
        assert!(
            verify_local_key(
                &status,
                &snapshot,
                Some(network),
                node,
                &"AB".repeat(32),
                true
            )
            .is_err()
        );
        assert!(
            verify_local_key(&status, &snapshot, Some(network), node, "invalid", true).is_err()
        );

        let mut drift = snapshot.clone();
        drift.current_network = Some(wrong_network);
        assert!(verify_local_key(&status, &drift, Some(network), node, &digest, true).is_err());
        drift.current_network = None;
        assert!(verify_local_key(&status, &drift, Some(network), node, &digest, true).is_err());
        let mut no_membership = status.clone();
        no_membership.current_network = None;
        assert!(
            verify_local_key(
                &no_membership,
                &snapshot,
                Some(network),
                node,
                &digest,
                true
            )
            .is_err()
        );
        let mut version_drift = snapshot.clone();
        version_drift.api_version = ApiVersion::new(1, 0);
        assert!(
            verify_local_key(&status, &version_drift, Some(network), node, &digest, true).is_err(),
            "cross-version status/diagnostics snapshots must not verify an identity"
        );
        let mut zero_network_status = status.clone();
        zero_network_status.current_network = Some(NetworkId::from_bytes([0; 16]));
        let mut zero_network_snapshot = snapshot.clone();
        zero_network_snapshot.current_network = zero_network_status.current_network;
        assert!(
            verify_local_key(
                &zero_network_status,
                &zero_network_snapshot,
                None,
                node,
                &digest,
                true,
            )
            .is_err(),
            "even without --network, an invalid local network must fail closed"
        );
        let mut bad_key = snapshot.clone();
        bad_key.local_control_tls_spki_sha256 = None;
        assert!(verify_local_key(&status, &bad_key, Some(network), node, &digest, true).is_err());

        // A not-yet-joined PANU can still independently verify its own
        // NodeId/key without pretending it already owns the target network.
        let mut joiner_status = status;
        joiner_status.current_network = None;
        let mut joiner_snapshot = snapshot;
        joiner_snapshot.current_network = None;
        let json =
            verify_local_key(&joiner_status, &joiner_snapshot, None, node, &digest, true).unwrap();
        let result: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(result["matches"], true);
        assert_eq!(result["network_id"], serde_json::Value::Null);
        assert_eq!(result["network_verified"], false);
        assert!(
            verify_local_key(
                &joiner_status,
                &joiner_snapshot,
                Some(network),
                node,
                &digest,
                true
            )
            .is_err()
        );
    }
}

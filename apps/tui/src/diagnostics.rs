//! Bounded read-only rendering of a single daemon diagnostic snapshot.
//! Never treat unknown/missing observations as a healthy empty inventory.
use blueroute_core::{NetworkBackend, NodeCapabilities, Sourced};
use blueroute_protocol::DiagnosticSnapshot;

const MAX_ROWS: usize = 8;
const MAX_FIELD_CHARS: usize = 120;

fn safe_field(value: &str) -> String {
    let mut result = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= MAX_FIELD_CHARS {
            result.push_str("...[truncated]");
            break;
        }
        if ch.is_control()
            || matches!(
                ch,
                '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
        {
            result.push_str(&format!("\\u{{{:x}}}", ch as u32));
        } else {
            result.push(ch);
        }
    }
    result
}

fn optional_field(value: Option<&str>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), safe_field)
}

pub(super) fn observed_count<T>(values: Option<&[T]>) -> String {
    match values {
        Some(values) => format!("{} reported", values.len()),
        None => "unavailable (not observed)".to_owned(),
    }
}

fn rows<T>(
    lines: &mut Vec<String>,
    name: &str,
    values: Option<&[T]>,
    describe: impl Fn(&T) -> String,
) {
    lines.push(format!("{name}: {}", observed_count(values)));
    if let Some(values) = values {
        for value in values.iter().take(MAX_ROWS) {
            lines.push(format!("  {}", describe(value)));
        }
        if values.len() > MAX_ROWS {
            lines.push(format!("  ... {} more omitted", values.len() - MAX_ROWS));
        }
    }
}

fn sourced_boolean(value: Option<&Sourced<bool>>) -> String {
    value.map_or_else(
        || "unknown (no local observation)".to_owned(),
        |fact| format!("{} ({:?})", fact.value, fact.source),
    )
}

fn capabilities(caps: &NodeCapabilities, lines: &mut Vec<String>) {
    lines.push("Local capabilities (not peer authentication):".to_owned());
    for (name, value) in [
        ("Adapter usable", caps.adapter_usable.as_ref()),
        ("Can join as client", caps.panu.as_ref()),
        ("Can host", caps.nap.as_ref()),
        ("Routing", caps.routing.as_ref()),
    ] {
        lines.push(format!("  {name}: {}", sourced_boolean(value)));
    }
    let backend = caps.network_backend.as_ref().map_or_else(
        || "unknown".to_owned(),
        |fact| {
            let value = match &fact.value {
                NetworkBackend::NetworkManager => "NetworkManager".to_owned(),
                NetworkBackend::SystemdNetworkd => "systemd-networkd".to_owned(),
                NetworkBackend::DirectNetlink => "direct netlink".to_owned(),
                NetworkBackend::Other(value) => format!("other: {}", safe_field(value)),
            };
            format!("{value} ({:?})", fact.source)
        },
    );
    lines.push(format!("  Network backend: {backend}"));
    let ceiling = caps.connection_policy_ceiling.as_ref().map_or_else(
        || "unknown".to_owned(),
        |fact| format!("{} ({:?})", fact.value, fact.source),
    );
    lines.push(format!(
        "  NAP link policy ceiling: {ceiling} (not a measured controller maximum)"
    ));
    // Internet sharing is future work: do not render its placeholder as a feature.
}

pub(super) fn format_diagnostics(snapshot: &DiagnosticSnapshot) -> String {
    let mut lines = vec![
        format!("API: {}", snapshot.api_version),
        format!("Local daemon health: {:?}", snapshot.health),
        format!(
            "Network: {}",
            snapshot
                .current_network
                .map_or_else(|| "none reported".to_owned(), |network| network.to_string())
        ),
        format!(
            "Visible nodes: {} (local-only until authenticated peer observation)",
            snapshot.visible_nodes
        ),
    ];
    capabilities(&snapshot.capabilities, &mut lines);
    match snapshot.system.as_ref() {
        None => lines.push("System: unavailable (not observed)".to_owned()),
        Some(system) => {
            lines.push(format!(
                "System: {} daemon={} kernel={} BlueZ={} network-backend={}",
                safe_field(&system.platform),
                safe_field(&system.daemon_version),
                optional_field(system.kernel_release.as_deref()),
                optional_field(system.bluez_version.as_deref()),
                optional_field(system.network_backend_version.as_deref()),
            ));
            rows(
                &mut lines,
                "Adapters",
                Some(system.adapters.as_slice()),
                |adapter| {
                    format!(
                        "id={} powered={} address={} driver={}",
                        safe_field(&adapter.id),
                        adapter.powered,
                        optional_field(adapter.address.as_deref()),
                        optional_field(adapter.driver.as_deref()),
                    )
                },
            );
        }
    }
    rows(
        &mut lines,
        "Observed peers (not authenticated membership)",
        snapshot.observed_peers.as_deref(),
        |peer| format!("{} {} ({:?})", peer.id, peer.name, peer.reachability),
    );
    rows(
        &mut lines,
        "Interfaces",
        snapshot.interfaces.as_deref(),
        |interface| {
            let addresses: Vec<String> = interface
                .addresses
                .iter()
                .take(MAX_ROWS)
                .map(|address| safe_field(address))
                .collect();
            format!(
                "name={} addresses=[{}]{}",
                safe_field(&interface.name),
                addresses.join(", "),
                if interface.addresses.len() > MAX_ROWS {
                    " ...[more omitted]"
                } else {
                    ""
                }
            )
        },
    );
    rows(&mut lines, "Routes", snapshot.routes.as_deref(), |route| {
        format!(
            "destination={:?} via={} cost={} owner={:?}",
            route.destination, route.next_hop, route.cost, route.owner
        )
    });
    match &snapshot.topology {
        None => lines.push("Topology: unavailable (not observed)".to_owned()),
        Some(topology) => {
            rows(
                &mut lines,
                "Topology nodes",
                Some(&topology.nodes),
                |node| format!("{} {} ({:?})", node.id, node.name, node.reachability),
            );
            rows(
                &mut lines,
                "Topology links",
                Some(&topology.links),
                |link| {
                    format!(
                        "id={} {} -> {} state={:?} health={:?}",
                        safe_field(&link.id),
                        link.left,
                        link.right,
                        link.state,
                        link.health
                    )
                },
            );
        }
    }
    rows(
        &mut lines,
        "Recent API failures",
        Some(&snapshot.recent_errors),
        |failure| {
            format!(
                "unix_ms={} operation={} class={}",
                failure.timestamp_unix_ms,
                safe_field(&failure.operation),
                safe_field(&failure.failure_class),
            )
        },
    );
    lines.push("Link transitions: unavailable (not exposed by daemon)".to_owned());
    lines.push(
        "Daemon observations only; not proof of authenticated peers or Bluetooth traffic."
            .to_owned(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::{
        CapabilitySource, DisplayName, HealthLevel, NetworkId, NodeId, Reachability, Route,
        RouteDestination, RouteOwner,
    };
    use blueroute_protocol::{
        ApiVersion, DiagnosticAdapter, DiagnosticFailure, DiagnosticInterface, DiagnosticTopology,
        DiagnosticTopologyLink, NodeSummary, SystemDiagnosticMetadata,
    };

    fn empty_snapshot() -> DiagnosticSnapshot {
        DiagnosticSnapshot {
            api_version: ApiVersion::CURRENT,
            health: HealthLevel::Healthy,
            current_network: None,
            visible_nodes: 1,
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
    fn missing_observations_do_not_become_healthy_zero_counts() {
        let text = format_diagnostics(&empty_snapshot());
        assert!(text.contains("Interfaces: unavailable (not observed)"));
        assert!(text.contains("Routes: unavailable (not observed)"));
        assert!(text.contains("Topology: unavailable (not observed)"));
        assert!(text.contains("Adapter usable: unknown"));
        assert!(text.contains("Link transitions: unavailable"));
        assert!(!text.contains("Internet sharing"));
    }

    #[test]
    fn rich_snapshot_renders_capabilities_interfaces_routes_and_topology() {
        let mut snapshot = empty_snapshot();
        let node = NodeId::from_bytes([4; 16]);
        let network = NetworkId::from_bytes([5; 16]);
        snapshot.current_network = Some(network);
        snapshot.capabilities.nap = Some(Sourced::new(true, CapabilitySource::Measured));
        snapshot.capabilities.network_backend = Some(Sourced::new(
            NetworkBackend::NetworkManager,
            CapabilitySource::Discovered,
        ));
        snapshot.system = Some(SystemDiagnosticMetadata {
            daemon_version: "0.1.0".into(),
            platform: "Linux".into(),
            kernel_release: Some("6.12".into()),
            bluez_version: Some("5.80".into()),
            network_backend_version: Some("1.50".into()),
            adapters: vec![DiagnosticAdapter {
                id: "hci0".into(),
                powered: true,
                address: None,
                driver: Some("btusb".into()),
            }],
        });
        snapshot.observed_peers = Some(Vec::new());
        snapshot.interfaces = Some(vec![DiagnosticInterface {
            name: "bnep0".into(),
            addresses: vec!["10.25.0.1/24".into()],
        }]);
        snapshot.routes = Some(vec![Route {
            destination: RouteDestination::Node(node),
            next_hop: node,
            cost: 10,
            owner: RouteOwner::BlueRouteNetwork(network),
        }]);
        snapshot.topology = Some(DiagnosticTopology {
            nodes: vec![NodeSummary {
                id: node,
                name: DisplayName::new("Local test").unwrap(),
                reachability: Reachability::Local,
            }],
            links: vec![DiagnosticTopologyLink {
                id: "simulated".into(),
                left: node,
                right: NodeId::from_bytes([6; 16]),
                state: blueroute_core::LinkState::Active,
                health: blueroute_core::LinkHealth::Healthy,
            }],
        });
        snapshot.recent_errors.push(DiagnosticFailure {
            timestamp_unix_ms: 42,
            operation: "get_diagnostics".into(),
            failure_class: "unsupported".into(),
        });
        let text = format_diagnostics(&snapshot);
        assert!(text.contains("Can host: true (Measured)"));
        assert!(text.contains("Network backend: NetworkManager (Discovered)"));
        assert!(text.contains("Adapters: 1 reported"));
        assert!(text.contains("hci0 powered=true"));
        assert!(text.contains("Interfaces: 1 reported"));
        assert!(text.contains("bnep0 addresses=[10.25.0.1/24]"));
        assert!(text.contains("Routes: 1 reported"));
        assert!(text.contains("Topology links: 1 reported"));
        assert!(text.contains("Recent API failures: 1 reported"));
        assert!(text.contains("operation=get_diagnostics class=unsupported"));
        assert!(text.contains("Observed peers (not authenticated membership): 0 reported"));
    }

    #[test]
    fn untrusted_strings_cannot_escape_terminal_or_expand_without_bound() {
        let mut snapshot = empty_snapshot();
        snapshot.recent_errors = (0..20)
            .map(|i| DiagnosticFailure {
                timestamp_unix_ms: i,
                operation: format!("{}\u{1b}[31m\u{202e}injected", "z".repeat(500)),
                failure_class: "bad\nspoofed: Healthy".to_owned(),
            })
            .collect();
        snapshot.interfaces = Some(vec![DiagnosticInterface {
            name: "bnep\u{1b}[2J".to_owned(),
            addresses: vec!["10.0.0.1/24".to_owned()],
        }]);
        let text = format_diagnostics(&snapshot);
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains('\u{202e}'));
        assert!(!text.contains("bad\nspoofed: Healthy"));
        assert!(text.contains("bad\\u{a}spoofed"));
        assert!(text.contains("...[truncated]"));
        assert!(text.contains("... 12 more omitted"));
        assert!(text.len() < 5000);
    }
}

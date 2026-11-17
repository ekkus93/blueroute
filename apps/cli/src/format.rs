//! Human and JSON rendering of daemon responses, escaping untrusted daemon-supplied text.

use blueroute_core::NodeCapabilities;
use blueroute_protocol::{
    ClientLinkState, DaemonStatus, DiagnosticSnapshot, NetworkSummary, NodeSummary,
    SystemDiagnosticMetadata,
};

pub(super) fn format_status(
    status: &DaemonStatus,
    json: bool,
) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(status)
    } else {
        Ok(format!(
            "API: {}\nLocal node: {:?}\nNetwork: {:?}\nHealth: {:?}",
            status.api_version, status.local_node, status.current_network, status.health
        ))
    }
}

pub(super) fn format_capabilities(
    caps: &NodeCapabilities,
    json: bool,
) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(caps)
    } else {
        let ceiling = match &caps.connection_policy_ceiling {
            Some(value) => format!(
                "{} (source: {:?}; policy ceiling, not a verified controller maximum)",
                value.value, value.source
            ),
            None => "unknown (new NAP admissions must fail closed)".to_owned(),
        };
        Ok(format!(
            "BlueRoute capabilities:\nNAP client policy ceiling: {ceiling}\nActive PANU count: unavailable (not reported by daemon)\n{caps:#?}"
        ))
    }
}

/// Human diagnostics print only explicit daemon observations. None means
/// "not available", which must not be confused with an observed empty list.
/// Bound the output independently of the JSON contract.
fn format_diagnostic_entries<T>(
    name: &str,
    entries: Option<&[T]>,
    describe: impl Fn(&T) -> String,
) -> String {
    const MAX_ROWS: usize = 32;
    match entries {
        None => format!("{name}: unavailable (not reported by daemon)"),
        Some([]) => format!("{name}: none reported by daemon"),
        Some(entries) => {
            let mut lines = vec![format!("{name}: {} daemon-reported", entries.len())];
            for entry in entries.iter().take(MAX_ROWS) {
                lines.push(format!("  {}", describe(entry)));
            }
            if entries.len() > MAX_ROWS {
                lines.push(format!(
                    "  ... {} additional entries omitted",
                    entries.len() - MAX_ROWS
                ));
            }
            lines.join("\n")
        }
    }
}

pub(super) fn format_diagnostics(
    snapshot: &DiagnosticSnapshot,
    json: bool,
) -> Result<String, serde_json::Error> {
    if json {
        return serde_json::to_string_pretty(snapshot);
    }
    let mut details = vec![format!(
        "API: {}\nHealth: {:?}\nNetwork: {:?}\nReported nodes: {} (this computer plus authenticated peers)\nLocal control TLS SPKI SHA-256: {}\nSystem: {}\nRecent API failures: {} (bounded details below)\nCapabilities:\n{:#?}",
        snapshot.api_version,
        snapshot.health,
        snapshot.current_network,
        snapshot.visible_nodes,
        snapshot
            .local_control_tls_spki_sha256
            .as_deref()
            .unwrap_or("unavailable"),
        snapshot.system.as_ref().map_or_else(
            || "platform details unavailable".to_owned(),
            format_system_diagnostics,
        ),
        snapshot.recent_errors.len(),
        snapshot.capabilities,
    )];
    details.push(format_diagnostic_entries(
        "Recent failure details",
        Some(snapshot.recent_errors.as_slice()),
        |failure| {
            format!(
                "unix_ms={} operation={} class={}",
                failure.timestamp_unix_ms,
                safe_diagnostic_field(&failure.operation),
                safe_diagnostic_field(&failure.failure_class)
            )
        },
    ));
    details.push(format_diagnostic_entries(
        "System adapters",
        snapshot
            .system
            .as_ref()
            .map(|system| system.adapters.as_slice()),
        |adapter| {
            format!(
                "id={:?} powered={} address={:?} driver={:?}",
                adapter.id, adapter.powered, adapter.address, adapter.driver
            )
        },
    ));
    details.push(format_diagnostic_entries(
        "Observed peers (daemon-reported; do not infer authentication)",
        snapshot.observed_peers.as_deref(),
        |peer| {
            format!(
                "{} name={:?} reachability={:?}",
                peer.id, peer.name, peer.reachability
            )
        },
    ));
    details.push(format_diagnostic_entries(
        &format!(
            "Authenticated control peers on this host ({})",
            control_peer_capacity(snapshot)
        ),
        snapshot.authenticated_control_peers.as_deref(),
        |peer| peer.to_string(),
    ));
    details.push(format_diagnostic_entries(
        "Interfaces",
        snapshot.interfaces.as_deref(),
        |interface| {
            format!(
                "name={:?} addresses={:?}",
                interface.name, interface.addresses
            )
        },
    ));
    details.push(format_diagnostic_entries(
        "Routes",
        snapshot.routes.as_deref(),
        |route| format!("{route:?}"),
    ));
    if let Some(topology) = &snapshot.topology {
        details.push(format_diagnostic_entries(
            "Topology nodes",
            Some(&topology.nodes),
            |node| {
                format!(
                    "{} name={:?} reachability={:?}",
                    node.id, node.name, node.reachability
                )
            },
        ));
        details.push(format_diagnostic_entries(
            "Topology links",
            Some(&topology.links),
            |link| {
                format!(
                    "id={:?} left={} right={} state={:?} health={:?}",
                    link.id, link.left, link.right, link.state, link.health
                )
            },
        ));
    } else {
        details.push("Topology: unavailable (not reported by daemon)".to_owned());
    }
    Ok(details.join("\n"))
}

/// "N of a policy ceiling of M (source)", labelled so it is never read as a measured radio limit
/// or as an observed PANU count.
fn control_peer_capacity(snapshot: &DiagnosticSnapshot) -> String {
    let Some(peers) = &snapshot.authenticated_control_peers else {
        return "unavailable (not reported by daemon)".to_owned();
    };
    let ceiling = snapshot
        .capabilities
        .connection_policy_ceiling
        .as_ref()
        .map_or_else(
            || "an unknown policy ceiling".to_owned(),
            |ceiling| {
                format!(
                    "a policy ceiling of {} ({:?}; not a measured controller limit)",
                    ceiling.value, ceiling.source
                )
            },
        );
    format!(
        "{} of {ceiling}; control sessions, not observed PANU links",
        peers.len()
    )
}

/// Short read-only health overview for automation and first-line support.
/// Missing observations are never displayed as observed zero, and discovery
/// counts never imply authenticated remote membership.
pub(super) fn format_diagnostics_summary(snapshot: &DiagnosticSnapshot) -> String {
    let count = |value: Option<usize>| match value {
        Some(value) => format!("{value} daemon-reported"),
        None => "unavailable (not reported by daemon)".to_owned(),
    };
    let topology = snapshot.topology.as_ref().map(|topology| {
        format!(
            "{} nodes, {} links (daemon-reported; not authentication evidence)",
            topology.nodes.len(),
            topology.links.len()
        )
    });
    format!(
        "Health: {:?}\nNetwork: {:?}\nReported nodes: {} (this computer plus authenticated peers)\nBluetooth adapters: {}\nObserved peers: {} (not authenticated membership)\nAuthenticated control peers: {}\nInterfaces: {}\nRoutes: {}\nTopology: {}\nRecent API failures: {}\nUse 'blueroute diagnostics show' for details or '--json' for the full snapshot.",
        snapshot.health,
        snapshot.current_network,
        snapshot.visible_nodes,
        count(snapshot.system.as_ref().map(|system| system.adapters.len())),
        count(snapshot.observed_peers.as_ref().map(Vec::len)),
        control_peer_capacity(snapshot),
        count(snapshot.interfaces.as_ref().map(Vec::len)),
        count(snapshot.routes.as_ref().map(Vec::len)),
        topology.unwrap_or_else(|| "unavailable (not reported by daemon)".to_owned()),
        snapshot.recent_errors.len(),
    )
}

/// Escape terminal controls and bidirectional formatting markers in externally
/// supplied diagnostic text. These fields are display-only and must never
/// inject new terminal lines or control sequences into CLI output.
fn safe_diagnostic_field(value: &str) -> String {
    // Avoid unbounded terminal output/allocation for daemon-supplied strings.
    // Cap on Unicode scalar values rather than bytes to preserve UTF-8.
    const MAX_DISPLAY_CHARS: usize = 256;
    let mut safe = String::with_capacity(value.len().min(MAX_DISPLAY_CHARS * 8));
    for (index, ch) in value.chars().enumerate() {
        if index == MAX_DISPLAY_CHARS {
            safe.push_str("...[truncated]");
            break;
        }
        if ch.is_control()
            || matches!(ch, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            safe.push_str(&format!("\\u{{{:x}}}", ch as u32));
        } else {
            safe.push(ch);
        }
    }
    safe
}

fn format_system_diagnostics(system: &SystemDiagnosticMetadata) -> String {
    format!(
        "platform={} daemon={} kernel={:?} BlueZ={:?} network-backend={:?} adapters={}",
        safe_diagnostic_field(&system.platform),
        safe_diagnostic_field(&system.daemon_version),
        system.kernel_release,
        system.bluez_version,
        system.network_backend_version,
        system.adapters.len()
    )
}

pub(super) fn format_networks(
    networks: &[NetworkSummary],
    json: bool,
) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(networks)
    } else if networks.is_empty() {
        Ok("No known BlueRoute networks".to_owned())
    } else {
        Ok(networks
            .iter()
            .map(|network| {
                format!(
                    "{}  {}  ({} members)",
                    network.id, network.name, network.member_count
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

pub(super) fn format_nodes(nodes: &[NodeSummary], json: bool) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(nodes)
    } else if nodes.is_empty() {
        Ok("No BlueRoute nodes reported by the daemon".to_owned())
    } else {
        Ok(nodes
            .iter()
            .map(|node| format!("{}  {}  ({:?})", node.id, node.name, node.reachability))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

pub(super) fn format_node(node: &NodeSummary, json: bool) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(node)
    } else {
        Ok(format!(
            "Node: {}\nName: {}\nReachability: {:?}",
            node.id, node.name, node.reachability
        ))
    }
}

/// Renders the joined-client link. The daemon's reason text is escaped like any other
/// diagnostic field so it cannot inject terminal controls.
pub(super) fn format_client_link(
    link: Option<&ClientLinkState>,
    json: bool,
) -> Result<String, serde_json::Error> {
    if json {
        return serde_json::to_string_pretty(&link);
    }
    Ok(match link {
        None => "This computer is not a joined client of a BlueRoute network.".to_owned(),
        Some(ClientLinkState::Connecting { attempt }) => {
            format!("Link: connecting to the host (attempt {attempt})")
        }
        Some(ClientLinkState::Waiting {
            attempt,
            retry_in_seconds,
        }) => format!("Link: attempt {attempt} failed; trying again in {retry_in_seconds} s"),
        Some(ClientLinkState::Established) => "Link: connected to the host".to_owned(),
        Some(ClientLinkState::NeedsAttention { reason }) => format!(
            "Link: needs attention: {}\nFix the cause, then run `blueroute network reconnect`.",
            safe_diagnostic_field(reason)
        ),
        Some(ClientLinkState::GaveUp { attempts }) => format!(
            "Link: stopped after {attempts} attempts; membership is kept.\n\
             Run `blueroute network reconnect` to try again."
        ),
    })
}

/// Trust operations are PolicyKit-gated by the daemon; CLI success is not
/// equivalent to TLS key enrollment or admission as a network member.
pub(super) fn mutation_ack(json: bool, success: &'static str) -> String {
    if json {
        r#"{"ok":true}"#.to_owned()
    } else {
        success.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use blueroute_core::{DisplayName, HealthLevel, NetworkId, NodeCapabilities, NodeId};

    use blueroute_protocol::{
        ApiVersion, ClientLinkState, DaemonStatus, DiagnosticSnapshot, NetworkSummary, NodeSummary,
        SystemDiagnosticMetadata,
    };

    #[test]
    fn system_diagnostics_escape_terminal_and_direction_controls() {
        let system = SystemDiagnosticMetadata {
            daemon_version: "1.0\nspoofed".to_owned(),
            platform: "Linux\u{1b}[31m\u{202e}".to_owned(),
            kernel_release: None,
            bluez_version: None,
            network_backend_version: None,
            adapters: Vec::new(),
        };
        let rendered = format_system_diagnostics(&system);
        assert!(rendered.contains("platform=Linux\\u{1b}[31m\\u{202e}"));
        assert!(rendered.contains("daemon=1.0\\u{a}spoofed"));
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{202e}'));
        assert!(!rendered.contains('\n'));
        assert_eq!(safe_diagnostic_field("plain Linux"), "plain Linux");
    }
    #[test]
    fn human_diagnostics_bound_large_untrusted_fields_without_truncating_utf8() {
        let large = format!("{}{}", "Ω".repeat(256), "\u{1b}[31mhidden");
        let sanitized = safe_diagnostic_field(&large);
        assert_eq!(sanitized, format!("{}...[truncated]", "Ω".repeat(256)));
        assert!(!sanitized.contains('\u{1b}'));
        assert!(!sanitized.contains("hidden"));
        assert_eq!(safe_diagnostic_field(&"Ω".repeat(256)), "Ω".repeat(256));

        let system = SystemDiagnosticMetadata {
            daemon_version: "0.1.0".into(),
            platform: large,
            kernel_release: None,
            bluez_version: None,
            network_backend_version: None,
            adapters: Vec::new(),
        };
        let output = format_system_diagnostics(&system);
        assert!(output.contains("...[truncated]"));
        assert!(!output.contains('\u{1b}'));
        assert!(!output.contains("hidden"));
        // The read-only JSON contract remains lossless.
        let encoded = serde_json::to_string(&system).unwrap();
        assert!(encoded.contains("hidden"));
    }
    #[test]
    fn client_link_renders_each_state_and_escapes_the_reason() {
        let text =
            |link: Option<ClientLinkState>| format_client_link(link.as_ref(), false).unwrap();
        assert!(text(None).contains("not a joined client"));
        assert!(text(Some(ClientLinkState::Connecting { attempt: 2 })).contains("attempt 2"));
        assert!(
            text(Some(ClientLinkState::Waiting {
                attempt: 3,
                retry_in_seconds: 8,
            }))
            .contains("in 8 s")
        );
        assert!(text(Some(ClientLinkState::Established)).contains("connected"));
        let gave_up = text(Some(ClientLinkState::GaveUp { attempts: 8 }));
        assert!(gave_up.contains("8 attempts") && gave_up.contains("network reconnect"));
        let attention = text(Some(ClientLinkState::NeedsAttention {
            reason: "host key missing\u{1b}[31m".into(),
        }));
        assert!(attention.contains("host key missing"));
        assert!(attention.contains("network reconnect"));
        assert!(!attention.contains('\u{1b}'));
        assert_eq!(format_client_link(None, true).unwrap(), "null");
        let encoded = format_client_link(Some(&ClientLinkState::GaveUp { attempts: 1 }), true);
        assert!(encoded.unwrap().contains("gave_up"));
    }
    #[test]
    fn formatters_use_daemon_data_without_model_specific_assumptions() {
        let status = DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([1; 16])),
            current_network: Some(NetworkId::from_bytes([2; 16])),
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        };
        let human = format_status(&status, false).unwrap();
        assert!(human.contains("Health: Healthy"));
        let json = format_status(&status, true).unwrap();
        let round_trip: DaemonStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(round_trip, status);
        let caps = NodeCapabilities::default();
        let caps_json = format_capabilities(&caps, true).unwrap();
        let parsed: NodeCapabilities = serde_json::from_str(&caps_json).unwrap();
        assert_eq!(parsed, caps);
    }
    #[test]
    fn nap_capacity_diagnostics_keep_policy_provenance_and_unknown_activity_distinct() {
        use blueroute_core::{CapabilitySource, Sourced};

        let mut caps = NodeCapabilities::default();
        let unknown = format_capabilities(&caps, false).unwrap();
        assert!(unknown.contains("NAP client policy ceiling: unknown"));
        assert!(unknown.contains("Active PANU count: unavailable"));

        caps.connection_policy_ceiling =
            Some(Sourced::new(4, CapabilitySource::ConservativeDefault));
        let human = format_capabilities(&caps, false).unwrap();
        assert!(human.contains("NAP client policy ceiling: 4"));
        assert!(human.contains("ConservativeDefault"));
        assert!(human.contains("not a verified controller maximum"));
        assert!(human.contains("Active PANU count: unavailable"));

        let json = format_capabilities(&caps, true).unwrap();
        let decoded: NodeCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, caps);
    }
    #[test]
    fn diagnostics_formatters_report_only_daemon_snapshot_fields() {
        let mut snapshot = DiagnosticSnapshot {
            api_version: ApiVersion::CURRENT,
            health: HealthLevel::Degraded,
            current_network: None,
            visible_nodes: 1,
            capabilities: NodeCapabilities::default(),
            local_control_tls_spki_sha256: None,
            system: None,
            observed_peers: None,
            interfaces: None,
            routes: None,
            topology: None,
            authenticated_control_peers: None,
            recent_errors: Vec::new(),
        };
        snapshot.system = Some(SystemDiagnosticMetadata {
            daemon_version: "0.1.0".into(),
            platform: "Linux".into(),
            kernel_release: Some("6.12.0".into()),
            bluez_version: Some("5.79".into()),
            network_backend_version: None,
            adapters: Vec::new(),
        });
        snapshot.local_control_tls_spki_sha256 = Some("ab".repeat(32));
        let output = format_diagnostics(&snapshot, false).unwrap();
        assert!(output.contains(&"ab".repeat(32)));
        assert!(output.contains("Health: Degraded"));
        assert!(output.contains("this computer plus authenticated peers"));
        assert!(output.contains("daemon=0.1.0"));
        assert!(!output.contains("verified remote peers"));
        let json = format_diagnostics(&snapshot, true).unwrap();
        let decoded: DiagnosticSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, snapshot);
    }
    #[test]
    fn human_diagnostics_distinguish_missing_from_observed_and_escape_untrusted_labels() {
        let mut snapshot = DiagnosticSnapshot {
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
            authenticated_control_peers: None,
            recent_errors: vec![],
        };
        let unknown = format_diagnostics(&snapshot, false).unwrap();
        assert!(unknown.contains("Interfaces: unavailable (not reported by daemon)"));
        assert!(unknown.contains("Routes: unavailable (not reported by daemon)"));
        assert!(unknown.contains("Topology: unavailable (not reported by daemon)"));
        assert!(unknown.contains(
            "Observed peers (daemon-reported; do not infer authentication): unavailable"
        ));
        snapshot.interfaces = Some(vec![blueroute_protocol::DiagnosticInterface {
            name: "bnep0\nunsafe".into(),
            addresses: vec!["10.99.1.1/24".into()],
        }]);
        snapshot.routes = Some(vec![]);
        snapshot.observed_peers = Some(vec![]);
        snapshot.topology = Some(blueroute_protocol::DiagnosticTopology {
            nodes: vec![],
            links: vec![],
        });
        let known = format_diagnostics(&snapshot, false).unwrap();
        assert!(known.contains("Interfaces: 1 daemon-reported"));
        assert!(known.contains("bnep0\\nunsafe"));
        assert!(!known.contains("bnep0\nunsafe"));
        assert!(known.contains("10.99.1.1/24"));
        assert!(known.contains("Routes: none reported by daemon"));
        assert!(known.contains("Topology nodes: none reported by daemon"));
        assert!(known.contains("Topology links: none reported by daemon"));
        assert!(known.contains(
            "Observed peers (daemon-reported; do not infer authentication): none reported by daemon"
        ));
        let encoded = format_diagnostics(&snapshot, true).unwrap();
        assert_eq!(
            serde_json::from_str::<DiagnosticSnapshot>(&encoded).unwrap(),
            snapshot
        );
    }
    #[test]
    fn detailed_diagnostics_escape_and_bound_recent_failure_records() {
        let mut snapshot = DiagnosticSnapshot {
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
            authenticated_control_peers: None,
            recent_errors: Vec::new(),
        };
        snapshot
            .recent_errors
            .push(blueroute_protocol::DiagnosticFailure {
                timestamp_unix_ms: 1234,
                operation: "join\\nspoof".replace("\\n", "\n"),
                failure_class: "Denied\\u{1b}[31m".replace("\\u{1b}", "\u{1b}"),
            });
        let detailed = format_diagnostics(&snapshot, false).unwrap();
        assert!(detailed.contains("Recent failure details: 1 daemon-reported"));
        assert!(detailed.contains("unix_ms=1234"));
        assert!(detailed.contains("operation=join\\u{a}spoof"));
        assert!(detailed.contains("class=Denied\\u{1b}[31m"));
        assert!(!detailed.contains("join\nspoof"));
        assert!(!detailed.contains('\u{1b}'));
        let json = format_diagnostics(&snapshot, true).unwrap();
        assert_eq!(
            serde_json::from_str::<DiagnosticSnapshot>(&json).unwrap(),
            snapshot
        );
        for i in 1..40 {
            snapshot
                .recent_errors
                .push(blueroute_protocol::DiagnosticFailure {
                    timestamp_unix_ms: i,
                    operation: format!("op{i}"),
                    failure_class: "denied".to_owned(),
                });
        }
        let bounded = format_diagnostics(&snapshot, false).unwrap();
        assert!(bounded.contains("Recent failure details: 40 daemon-reported"));
        assert!(bounded.contains("8 additional entries omitted"));
        assert!(!bounded.contains("operation=op39"));
    }
    #[test]
    fn human_diagnostics_bound_long_interface_inventory_without_changing_json() {
        let interfaces = (0..40)
            .map(|i| blueroute_protocol::DiagnosticInterface {
                name: format!("bnep{i}"),
                addresses: vec![],
            })
            .collect::<Vec<_>>();
        let snapshot = DiagnosticSnapshot {
            api_version: ApiVersion::CURRENT,
            health: HealthLevel::Healthy,
            current_network: None,
            visible_nodes: 0,
            capabilities: NodeCapabilities::default(),
            local_control_tls_spki_sha256: None,
            system: None,
            observed_peers: None,
            interfaces: Some(interfaces),
            routes: None,
            topology: None,
            authenticated_control_peers: None,
            recent_errors: vec![],
        };
        let human = format_diagnostics(&snapshot, false).unwrap();
        assert!(human.contains("Interfaces: 40 daemon-reported"));
        assert!(human.contains("8 additional entries omitted"));
        assert!(human.contains("bnep31"));
        assert!(!human.contains("bnep32"));
        let json = format_diagnostics(&snapshot, true).unwrap();
        assert!(json.contains("bnep39"));
    }
    #[test]
    fn control_peers_render_against_the_labelled_policy_ceiling() {
        use blueroute_core::{CapabilitySource, Sourced};
        let mut snapshot = DiagnosticSnapshot {
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
            authenticated_control_peers: None,
            recent_errors: Vec::new(),
        };
        // Not reported is never shown as zero.
        let missing = format_diagnostics_summary(&snapshot);
        assert!(missing.contains("Authenticated control peers: unavailable"));

        let peer = NodeId::from_bytes([0x31; 16]);
        snapshot.authenticated_control_peers = Some(vec![peer]);
        snapshot.capabilities.connection_policy_ceiling =
            Some(Sourced::new(4, CapabilitySource::ConservativeDefault));
        let summary = format_diagnostics_summary(&snapshot);
        assert!(summary.contains("1 of a policy ceiling of 4 (ConservativeDefault"));
        assert!(summary.contains("not a measured controller limit"));
        assert!(summary.contains("not observed PANU links"));
        let detail = format_diagnostics(&snapshot, false).unwrap();
        assert!(detail.contains(&peer.to_string()));

        snapshot.capabilities.connection_policy_ceiling = None;
        snapshot.authenticated_control_peers = Some(Vec::new());
        assert!(format_diagnostics_summary(&snapshot).contains("0 of an unknown policy ceiling"));
        let json = format_diagnostics(&snapshot, true).unwrap();
        assert!(json.contains("\"authenticated_control_peers\": []"));
    }
    #[test]
    fn read_only_node_inspection_renders_exact_daemon_supplied_data() {
        let node = NodeSummary {
            id: NodeId::from_bytes([9; 16]),
            name: DisplayName::new("Peer nine").unwrap(),
            reachability: blueroute_core::Reachability::Unreachable,
        };
        let list = vec![node.clone()];
        assert!(format_nodes(&list, false).unwrap().contains("Peer nine"));
        let decoded: Vec<NodeSummary> =
            serde_json::from_str(&format_nodes(&list, true).unwrap()).unwrap();
        assert_eq!(decoded, list);
        let human = format_node(&node, false).unwrap();
        assert!(human.contains("Unreachable"));
        let decoded: NodeSummary =
            serde_json::from_str(&format_node(&node, true).unwrap()).unwrap();
        assert_eq!(decoded, node);
        assert!(
            format_nodes(&[], false)
                .unwrap()
                .contains("No BlueRoute nodes")
        );
        assert_eq!(format_nodes(&[], true).unwrap(), "[]");
    }
    #[test]
    fn network_listing_is_stable_in_human_and_json_modes() {
        let networks = vec![NetworkSummary {
            id: NetworkId::from_bytes([7; 16]),
            name: DisplayName::new("Workshop").unwrap(),
            member_count: 2,
        }];
        let human = format_networks(&networks, false).unwrap();
        assert!(human.contains("Workshop"));
        assert!(human.contains("2 members"));
        let json = format_networks(&networks, true).unwrap();
        let restored: Vec<NetworkSummary> = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, networks);
        assert!(format_networks(&[], false).unwrap().contains("No known"));
    }
}

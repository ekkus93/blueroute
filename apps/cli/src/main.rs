//! BlueRoute CLI commands through the shared version-checked daemon client.

use blueroute_client::{BlueRouteClient, ClientError, ClientErrorKind};
use blueroute_core::{DisplayName, NetworkId, NodeCapabilities, NodeId};
use blueroute_protocol::peer_pin::SpkiSha256;
use blueroute_protocol::{
    Command, DaemonStatus, DiagnosticSnapshot, NetworkSummary, NodeSummary, Response,
    SystemDiagnosticMetadata,
};
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "blueroute", version, about = "Manage BlueRoute networks")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    Status,
    Capability {
        #[command(subcommand)]
        command: CapabilityCommand,
    },
    Diagnostics {
        #[command(subcommand)]
        command: DiagnosticsCommand,
    },
    Network {
        #[command(subcommand)]
        command: NetworkCommand,
    },
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
    Discover {
        #[command(subcommand)]
        command: DiscoverCommand,
    },
}

#[derive(Debug, Subcommand)]
enum CapabilityCommand {
    Show,
}
#[derive(Debug, Subcommand)]
enum DiagnosticsCommand {
    Show {
        #[arg(long, conflicts_with = "json")]
        summary: bool,
    },
}
#[derive(Debug, Subcommand)]
enum NetworkCommand {
    List,
    Create { name: String },
    Join { network: String },
    Leave,
}
#[derive(Debug, Subcommand)]
enum NodeCommand {
    List,
    Show { node: String },
}
#[derive(Debug, Subcommand)]
enum PeerCommand {
    LocalKey,
    Trust { node: String },
    Forget { node: String },
}
#[derive(Debug, Subcommand)]
enum DiscoverCommand {
    Start,
    Stop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Operation {
    Help,
    Version,
    Status { json: bool },
    Capabilities { json: bool },
    Diagnostics { json: bool, summary: bool },
    LocalKey { json: bool },
    NetworkList { json: bool },
    NodeList { json: bool },
    NodeShow { node: NodeId, json: bool },
    TrustPeer { node: NodeId, json: bool },
    ForgetPeer { node: NodeId, json: bool },
    CreateNetwork { name: DisplayName, json: bool },
    JoinNetwork { network: NetworkId, json: bool },
    LeaveNetwork { json: bool },
    StartDiscovery { json: bool },
    StopDiscovery { json: bool },
}

// Reject the all-zero sentinel before sending node-directed mutations to D-Bus.
fn parse_node_id(value: &str) -> Result<NodeId, &'static str> {
    let node: NodeId = value.parse().map_err(|_| "invalid full node NodeId")?;
    if node == NodeId::from_bytes([0; 16]) {
        return Err("zero NodeId is not a valid peer identity");
    }
    Ok(node)
}

fn parse_network_id(value: &str) -> Result<NetworkId, &'static str> {
    let network: NetworkId = value.parse().map_err(|_| "invalid full network ID")?;
    if network == NetworkId::from_bytes([0; 16]) {
        return Err("zero NetworkId is not a valid network identity");
    }
    Ok(network)
}

fn parse(args: &[String]) -> Result<Operation, String> {
    let cli = match Cli::try_parse_from(
        std::iter::once("blueroute".to_owned()).chain(args.iter().cloned()),
    ) {
        Ok(cli) => cli,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            return Ok(Operation::Help);
        }
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayVersion => {
            return Ok(Operation::Version);
        }
        Err(error) => return Err(error.to_string()),
    };
    let json = cli.json;
    let operation = match cli.command {
        None => return Ok(Operation::Help),
        Some(CliCommand::Status) => Operation::Status { json },
        Some(CliCommand::Capability {
            command: CapabilityCommand::Show,
        }) => Operation::Capabilities { json },
        Some(CliCommand::Diagnostics {
            command: DiagnosticsCommand::Show { summary },
        }) => Operation::Diagnostics { json, summary },
        Some(CliCommand::Network {
            command: NetworkCommand::List,
        }) => Operation::NetworkList { json },
        Some(CliCommand::Network {
            command: NetworkCommand::Create { name },
        }) => {
            if name.chars().any(char::is_control) {
                return Err("network name cannot contain control characters".into());
            }
            Operation::CreateNetwork {
                name: DisplayName::new(name).map_err(|_| "invalid network name")?,
                json,
            }
        }
        Some(CliCommand::Network {
            command: NetworkCommand::Join { network },
        }) => Operation::JoinNetwork {
            network: parse_network_id(&network).map_err(str::to_owned)?,
            json,
        },
        Some(CliCommand::Network {
            command: NetworkCommand::Leave,
        }) => Operation::LeaveNetwork { json },
        Some(CliCommand::Node {
            command: NodeCommand::List,
        }) => Operation::NodeList { json },
        Some(CliCommand::Node {
            command: NodeCommand::Show { node },
        }) => Operation::NodeShow {
            node: parse_node_id(&node).map_err(str::to_owned)?,
            json,
        },
        Some(CliCommand::Peer {
            command: PeerCommand::LocalKey,
        }) => Operation::LocalKey { json },
        Some(CliCommand::Peer {
            command: PeerCommand::Trust { node },
        }) => Operation::TrustPeer {
            node: parse_node_id(&node).map_err(str::to_owned)?,
            json,
        },
        Some(CliCommand::Peer {
            command: PeerCommand::Forget { node },
        }) => Operation::ForgetPeer {
            node: parse_node_id(&node).map_err(str::to_owned)?,
            json,
        },
        Some(CliCommand::Discover {
            command: DiscoverCommand::Start,
        }) => Operation::StartDiscovery { json },
        Some(CliCommand::Discover {
            command: DiscoverCommand::Stop,
        }) => Operation::StopDiscovery { json },
    };
    Ok(operation)
}

fn format_status(status: &DaemonStatus, json: bool) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(status)
    } else {
        Ok(format!(
            "API: {}\nLocal node: {:?}\nNetwork: {:?}\nHealth: {:?}",
            status.api_version, status.local_node, status.current_network, status.health
        ))
    }
}

fn format_capabilities(caps: &NodeCapabilities, json: bool) -> Result<String, serde_json::Error> {
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

fn format_diagnostics(
    snapshot: &DiagnosticSnapshot,
    json: bool,
) -> Result<String, serde_json::Error> {
    if json {
        return serde_json::to_string_pretty(snapshot);
    }
    let mut details = vec![format!(
        "API: {}\nHealth: {:?}\nNetwork: {:?}\nReported nodes: {} (local-only until peer authentication)\nLocal control TLS SPKI SHA-256: {}\nSystem: {}\nRecent API failures: {} (bounded details below)\nCapabilities:\n{:#?}",
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

/// Short read-only health overview for automation and first-line support.
/// Missing observations are never displayed as observed zero, and discovery
/// counts never imply authenticated remote membership.
fn format_diagnostics_summary(snapshot: &DiagnosticSnapshot) -> String {
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
        "Health: {:?}\nNetwork: {:?}\nReported nodes: {} (local-only until peer authentication)\nBluetooth adapters: {}\nObserved peers: {} (not authenticated membership)\nInterfaces: {}\nRoutes: {}\nTopology: {}\nRecent API failures: {}\nUse 'blueroute diagnostics show' for details or '--json' for the full snapshot.",
        snapshot.health,
        snapshot.current_network,
        snapshot.visible_nodes,
        count(snapshot.system.as_ref().map(|system| system.adapters.len())),
        count(snapshot.observed_peers.as_ref().map(Vec::len)),
        count(snapshot.interfaces.as_ref().map(Vec::len)),
        count(snapshot.routes.as_ref().map(Vec::len)),
        topology.unwrap_or_else(|| "unavailable (not reported by daemon)".to_owned()),
        snapshot.recent_errors.len(),
    )
}

/// Public local identity for out-of-band comparison. It is neither a trust
/// decision nor evidence of a remote certificate's private-key possession.
fn format_local_key(
    local_node: Option<NodeId>,
    fingerprint: Option<&str>,
    json: bool,
) -> Result<String, &'static str> {
    let node = local_node
        .filter(|node| *node != NodeId::from_bytes([0; 16]))
        .ok_or("daemon has no valid local NodeId")?;
    let fingerprint = SpkiSha256::parse_canonical_hex(
        fingerprint.ok_or("daemon has no local TLS public-key fingerprint")?,
    )
    .map_err(|_| "daemon supplied an invalid TLS public-key fingerprint")?
    .to_canonical_hex();
    if json {
        Ok(serde_json::json!({ "local_node": node, "spki_sha256": fingerprint }).to_string())
    } else {
        Ok(format!(
            "Local NodeId: {node}\nLocal control TLS SPKI SHA-256: {fingerprint}\nCompare these values through an independent channel. This does not enroll or authorize a peer."
        ))
    }
}

/// Escape terminal controls and bidirectional formatting markers in externally
/// supplied diagnostic text. These fields are display-only and must never
/// inject new terminal lines or control sequences into CLI output.
fn safe_diagnostic_field(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for ch in value.chars() {
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

fn format_networks(networks: &[NetworkSummary], json: bool) -> Result<String, serde_json::Error> {
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

fn format_nodes(nodes: &[NodeSummary], json: bool) -> Result<String, serde_json::Error> {
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

fn format_node(node: &NodeSummary, json: bool) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(node)
    } else {
        Ok(format!(
            "Node: {}\nName: {}\nReachability: {:?}",
            node.id, node.name, node.reachability
        ))
    }
}

/// Trust operations are PolicyKit-gated by the daemon; CLI success is not
/// equivalent to TLS key enrollment or admission as a network member.
fn mutation_ack(json: bool, success: &'static str) -> String {
    if json {
        r#"{"ok":true}"#.to_owned()
    } else {
        success.to_owned()
    }
}

/// Stable process exit codes: 1 operational, 2 usage, 3 unavailable,
/// 4 authorization failure, 5 unsupported capability.
#[derive(Debug)]
struct CliFailure {
    code: i32,
    message: String,
}

impl From<ClientError> for CliFailure {
    fn from(error: ClientError) -> Self {
        let code = match error.kind() {
            ClientErrorKind::Unavailable => 3,
            ClientErrorKind::Authorization => 4,
            ClientErrorKind::Unsupported => 5,
            ClientErrorKind::Operational => 1,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

impl From<serde_json::Error> for CliFailure {
    fn from(error: serde_json::Error) -> Self {
        Self {
            code: 1,
            message: error.to_string(),
        }
    }
}

impl From<String> for CliFailure {
    fn from(message: String) -> Self {
        Self { code: 1, message }
    }
}
impl From<&'static str> for CliFailure {
    fn from(message: &'static str) -> Self {
        Self {
            code: 1,
            message: message.to_owned(),
        }
    }
}

fn run(args: &[String]) -> i32 {
    let operation = match parse(args) {
        Ok(operation) => operation,
        Err(message) => {
            eprintln!("blueroute: {message}");
            return 2;
        }
    };
    match &operation {
        Operation::Help => {
            println!("{}", Cli::command().render_help());
            return 0;
        }
        Operation::Version => {
            println!("blueroute {}", env!("CARGO_PKG_VERSION"));
            return 0;
        }
        _ => {}
    }
    // This uses the shared system D-Bus API only; there are no direct
    // BlueZ, NetworkManager or privileged networking calls in the CLI.
    let client = match futures_lite::future::block_on(BlueRouteClient::connect()) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("blueroute: daemon unavailable/incompatible: {error}");
            return 3;
        }
    };
    let result = match operation {
        Operation::Status { json } => futures_lite::future::block_on(client.status())
            .map_err(CliFailure::from)
            .and_then(|status| format_status(&status, json).map_err(CliFailure::from)),
        Operation::Capabilities { json } => futures_lite::future::block_on(client.capabilities())
            .map_err(CliFailure::from)
            .and_then(|caps| format_capabilities(&caps, json).map_err(CliFailure::from)),
        Operation::Diagnostics { json, summary } => {
            futures_lite::future::block_on(client.request(&Command::GetDiagnostics))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Diagnostics(snapshot) => {
                        if summary {
                            Ok(format_diagnostics_summary(&snapshot))
                        } else {
                            format_diagnostics(&snapshot, json).map_err(CliFailure::from)
                        }
                    }
                    _ => Err("daemon returned an unexpected diagnostics response".into()),
                })
        }
        Operation::LocalKey { json } => futures_lite::future::block_on(client.status())
            .map_err(CliFailure::from)
            .and_then(|status| {
                futures_lite::future::block_on(client.request(&Command::GetDiagnostics))
                    .map_err(CliFailure::from)
                    .and_then(|response| match response {
                        Response::Diagnostics(snapshot) => format_local_key(
                            status.local_node,
                            snapshot.local_control_tls_spki_sha256.as_deref(),
                            json,
                        )
                        .map_err(CliFailure::from),
                        _ => Err("daemon returned an unexpected local-key response".into()),
                    })
            }),
        Operation::NetworkList { json } => {
            futures_lite::future::block_on(client.request(&Command::ListNetworks))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Networks(networks) => {
                        format_networks(&networks, json).map_err(CliFailure::from)
                    }
                    _ => Err("daemon returned an unexpected network-list response".into()),
                })
        }
        Operation::NodeList { json } => {
            futures_lite::future::block_on(client.request(&Command::ListNodes))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Nodes(nodes) => format_nodes(&nodes, json).map_err(CliFailure::from),
                    _ => Err("daemon returned an unexpected node-list response".into()),
                })
        }
        Operation::NodeShow { node, json } => {
            futures_lite::future::block_on(client.request(&Command::GetNode { node }))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Node(Some(node)) => {
                        format_node(&node, json).map_err(CliFailure::from)
                    }
                    Response::Node(None) => Err("the daemon has no record for that NodeId".into()),
                    _ => Err("daemon returned an unexpected node-detail response".into()),
                })
        }
        Operation::TrustPeer { node, json } => {
            futures_lite::future::block_on(client.request(&Command::TrustPeer { node }))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Peer trust approved")),
                    _ => Err("daemon returned an unexpected trust response".into()),
                })
        }
        Operation::ForgetPeer { node, json } => {
            futures_lite::future::block_on(client.request(&Command::ForgetPeer { node }))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Peer trust revoked")),
                    _ => Err("daemon returned an unexpected forget response".into()),
                })
        }
        Operation::CreateNetwork { name, json } => {
            futures_lite::future::block_on(client.request(&Command::CreateNetwork { name }))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Network created")),
                    _ => Err("daemon returned an unexpected network-create response".into()),
                })
        }
        Operation::JoinNetwork { network, json } => {
            futures_lite::future::block_on(client.request(&Command::JoinNetwork { network }))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Joined BlueRoute network")),
                    _ => Err("daemon returned an unexpected network-join response".into()),
                })
        }
        Operation::LeaveNetwork { json } => {
            futures_lite::future::block_on(client.request(&Command::LeaveNetwork))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Left BlueRoute network")),
                    _ => Err("daemon returned an unexpected network-leave response".into()),
                })
        }
        Operation::StartDiscovery { json } => {
            futures_lite::future::block_on(client.request(&Command::StartDiscovery))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Discovery started")),
                    _ => Err("daemon returned an unexpected discovery response".into()),
                })
        }
        Operation::StopDiscovery { json } => {
            futures_lite::future::block_on(client.request(&Command::StopDiscovery))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Discovery stopped")),
                    _ => Err("daemon returned an unexpected discovery response".into()),
                })
        }
        Operation::Help | Operation::Version => unreachable!(),
    };
    match result {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("blueroute: {}", error.message);
            error.code
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::{DisplayName, HealthLevel, NetworkId, NodeId};
    use blueroute_protocol::ApiVersion;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).into()).collect()
    }

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
    fn parses_safe_read_only_commands_and_reports_bad_arguments() {
        assert_eq!(parse(&[]), Ok(Operation::Help));
        assert_eq!(parse(&args(&["--version"])), Ok(Operation::Version));
        assert_eq!(
            parse(&args(&["status"])),
            Ok(Operation::Status { json: false })
        );
        assert_eq!(
            parse(&args(&["--json", "capability", "show"])),
            Ok(Operation::Capabilities { json: true })
        );
        assert!(parse(&args(&["--json", "--json", "status"])).is_err());
        assert_eq!(
            parse(&args(&["network", "list", "--json"])),
            Ok(Operation::NetworkList { json: true })
        );
        assert_eq!(
            parse(&args(&["--json", "network", "leave"])),
            Ok(Operation::LeaveNetwork { json: true })
        );
        assert_eq!(
            parse(&args(&["diagnostics", "show", "--json"])),
            Ok(Operation::Diagnostics {
                json: true,
                summary: false,
            })
        );
        assert!(parse(&args(&["diagnostics"])).is_err());
        assert!(parse(&args(&["network", "join"])).is_err());
        let network = NetworkId::from_bytes([0x44; 16]);
        assert_eq!(
            parse(&args(&["network", "join", &network.to_string()])),
            Ok(Operation::JoinNetwork {
                network,
                json: false
            })
        );
        assert!(
            parse(&args(&[
                "network",
                "join",
                &NetworkId::from_bytes([0; 16]).to_string()
            ]))
            .is_err()
        );
        assert_eq!(
            parse(&args(&["node", "list", "--json"])),
            Ok(Operation::NodeList { json: true })
        );
        assert!(parse(&args(&["node", "show"])).is_err());
        assert!(parse(&args(&["node", "show", "bluetooth-address"])).is_err());
        let node = NodeId::from_bytes([3; 16]);
        let id = node.to_string();
        assert_eq!(
            parse(&args(&["node", "show", &id])),
            Ok(Operation::NodeShow { node, json: false })
        );
        assert_eq!(
            parse(&args(&["peer", "trust", &id])),
            Ok(Operation::TrustPeer { node, json: false })
        );
        assert_eq!(
            parse(&args(&["--json", "peer", "forget", &id])),
            Ok(Operation::ForgetPeer { node, json: true })
        );
        let invalid_node = NodeId::from_bytes([0; 16]).to_string();
        for command in [
            args(&["node", "show", &invalid_node]),
            args(&["peer", "trust", &invalid_node]),
            args(&["peer", "forget", &invalid_node]),
        ] {
            assert!(parse(&command).is_err(), "zero NodeId must fail locally");
        }
        assert_eq!(
            parse(&args(&["peer", "local-key", "--json"])),
            Ok(Operation::LocalKey { json: true })
        );
        assert!(parse(&args(&["peer", "local-key", "extra"])).is_err());
        assert!(parse(&args(&["peer", "trust"])).is_err());
        assert!(parse(&args(&["peer", "trust", "unknown-peer"])).is_err());
        assert!(parse(&args(&["peer", "forget", &id, "extra"])).is_err());
        assert_eq!(
            parse(&args(&["network", "create", "Workshop"])),
            Ok(Operation::CreateNetwork {
                name: DisplayName::new("Workshop").unwrap(),
                json: false
            })
        );
        assert_eq!(
            parse(&args(&["--json", "discover", "start"])),
            Ok(Operation::StartDiscovery { json: true })
        );
        assert_eq!(
            parse(&args(&["discover", "stop"])),
            Ok(Operation::StopDiscovery { json: false })
        );
        assert!(parse(&args(&["network", "create", ""])).is_err());
        assert!(parse(&args(&["network", "create", "bad\nname"])).is_err());
        assert!(parse(&args(&["discover"])).is_err());
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
        assert!(output.contains("local-only until peer authentication"));
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
            recent_errors: Vec::new(),
        };
        snapshot.recent_errors.push(blueroute_protocol::DiagnosticFailure {
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
            snapshot.recent_errors.push(blueroute_protocol::DiagnosticFailure {
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
    fn diagnostic_summary_is_opt_in_and_preserves_unknown_vs_empty() {
        assert_eq!(
            parse(&args(&["diagnostics", "show", "--summary"])),
            Ok(Operation::Diagnostics {
                json: false,
                summary: true,
            })
        );
        assert!(parse(&args(&["diagnostics", "show", "--summary", "--json"])).is_err());
        assert!(parse(&args(&["diagnostics", "show", "--summary", "--summary"])).is_err());
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
            recent_errors: Vec::new(),
        };
        let unknown = format_diagnostics_summary(&snapshot);
        assert!(unknown.contains("Health: Degraded"));
        assert!(unknown.contains("Interfaces: unavailable (not reported by daemon)"));
        assert!(unknown.contains("Bluetooth adapters: unavailable"));
        assert!(unknown.contains("Observed peers: unavailable"));
        assert!(unknown.contains("local-only until peer authentication"));
        snapshot.interfaces = Some(Vec::new());
        snapshot.routes = Some(Vec::new());
        snapshot.observed_peers = Some(Vec::new());
        snapshot.system = Some(SystemDiagnosticMetadata {
            daemon_version: "1.0".into(),
            platform: "Linux".into(),
            kernel_release: None,
            bluez_version: None,
            network_backend_version: None,
            adapters: Vec::new(),
        });
        let empty = format_diagnostics_summary(&snapshot);
        assert!(empty.contains("Interfaces: 0 daemon-reported"));
        assert!(empty.contains("Routes: 0 daemon-reported"));
        assert!(empty.contains("Observed peers: 0 daemon-reported"));
        assert!(empty.contains("Bluetooth adapters: 0 daemon-reported"));
        assert!(!empty.contains("authenticated remote peers"));
    }

    #[test]
    fn local_key_display_requires_both_valid_daemon_values_and_never_enrolls() {
        let node = NodeId::from_bytes([7; 16]);
        let digest = "ab".repeat(32);
        let human = format_local_key(Some(node), Some(&digest), false).unwrap();
        assert!(human.contains(&node.to_string()));
        assert!(human.contains(&digest));
        assert!(human.contains("does not enroll or authorize"));
        let encoded = format_local_key(Some(node), Some(&digest), true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(parsed["local_node"], node.to_string());
        assert_eq!(parsed["spki_sha256"], digest);
        assert!(format_local_key(None, Some(&digest), false).is_err());
        assert!(format_local_key(Some(NodeId::from_bytes([0; 16])), Some(&digest), false).is_err());
        assert!(format_local_key(Some(node), None, true).is_err());
        assert!(format_local_key(Some(node), Some(&"00".repeat(32)), true).is_err());
        assert!(format_local_key(Some(node), Some(&"AB".repeat(32)), true).is_err());
        assert!(format_local_key(Some(node), Some("invalid"), false).is_err());
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
    fn cli_error_exit_classes_are_stable_and_do_not_leak_unverified_peer_claims() {
        let unavailable = CliFailure::from(ClientError::IncompatibleVersion {
            client: ApiVersion::new(1, 0),
            server: ApiVersion::new(2, 0),
        });
        assert_eq!(unavailable.code, 3);
        let denied = CliFailure::from(ClientError::Bus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::AccessDenied("authorization denied".into()),
        ))));
        assert_eq!(denied.code, 4);
        let unsupported = CliFailure::from(ClientError::Bus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::NotSupported("unsupported runtime capability".into()),
        ))));
        assert_eq!(unsupported.code, 5);
        assert_eq!(CliFailure::from("unrecognized daemon response").code, 1);
    }

    #[test]
    fn explicit_mutation_commands_never_claim_membership_or_key_enrollment() {
        assert_eq!(
            mutation_ack(false, "Peer trust approved"),
            "Peer trust approved"
        );
        assert_eq!(mutation_ack(true, "Peer trust revoked"), r#"{"ok":true}"#);
        assert!(parse(&args(&["join"])).is_err());
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

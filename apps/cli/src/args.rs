//! The command tree and argument parsing: what the user asked for, as an `Operation`.

use blueroute_core::{DisplayName, NetworkId, NodeId};
use blueroute_protocol::peer_pin::SpkiSha256;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "blueroute", version, about = "Manage BlueRoute networks")]
pub(super) struct Cli {
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
    Create {
        name: String,
    },
    Join {
        network: String,
    },
    Leave,
    /// Show the joined client's link to its host (read-only).
    Link,
    /// Try again to reconnect a joined client to its host after it gave up or needed attention.
    Reconnect,
}
#[derive(Debug, Subcommand)]
enum NodeCommand {
    List,
    Show {
        node: String,
    },
    /// Change this computer's display name. It is a label only; the NodeId never changes.
    SetName {
        name: String,
    },
}
#[derive(Debug, Subcommand)]
enum PeerCommand {
    LocalKey,
    /// Compare an independently obtained identity tuple on the peer's own daemon.
    /// This is read-only and never enrolls a key.
    VerifyLocalKey {
        node: String,
        fingerprint: String,
        #[arg(long)]
        network: Option<String>,
    },
    Trust {
        node: String,
    },
    /// On the host: trust a joining computer and pin its key. Read both values on the joining
    /// computer with `blueroute peer local-key` and compare them out of band.
    Enroll {
        node: String,
        fingerprint: String,
    },
    Forget {
        node: String,
    },
    /// Record the one host you confirmed out of band (compare its fingerprint on the host with
    /// `blueroute peer local-key`). This does not join a network or grant membership.
    SetJoinHost {
        network: String,
        node: String,
        fingerprint: String,
    },
    /// Forget the recorded join host.
    ClearJoinHost,
}
#[derive(Debug, Subcommand)]
enum DiscoverCommand {
    Start,
    Stop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Operation {
    Help,
    Version,
    Status {
        json: bool,
    },
    Capabilities {
        json: bool,
    },
    Diagnostics {
        json: bool,
        summary: bool,
    },
    LocalKey {
        json: bool,
    },
    VerifyLocalKey {
        network: Option<NetworkId>,
        node: NodeId,
        fingerprint: String,
        json: bool,
    },
    NetworkList {
        json: bool,
    },
    NodeList {
        json: bool,
    },
    NodeShow {
        node: NodeId,
        json: bool,
    },
    SetDeviceName {
        name: DisplayName,
        json: bool,
    },
    TrustPeer {
        node: NodeId,
        json: bool,
    },
    EnrollPeer {
        node: NodeId,
        fingerprint: String,
        json: bool,
    },
    ForgetPeer {
        node: NodeId,
        json: bool,
    },
    SetJoinHost {
        network: NetworkId,
        node: NodeId,
        fingerprint: String,
        json: bool,
    },
    ClearJoinHost {
        json: bool,
    },
    CreateNetwork {
        name: DisplayName,
        json: bool,
    },
    JoinNetwork {
        network: NetworkId,
        json: bool,
    },
    LeaveNetwork {
        json: bool,
    },
    ClientLink {
        json: bool,
    },
    RetryClientReconnect {
        json: bool,
    },
    StartDiscovery {
        json: bool,
    },
    StopDiscovery {
        json: bool,
    },
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

pub(super) fn parse(args: &[String]) -> Result<Operation, String> {
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
        Some(CliCommand::Network {
            command: NetworkCommand::Link,
        }) => Operation::ClientLink { json },
        Some(CliCommand::Network {
            command: NetworkCommand::Reconnect,
        }) => Operation::RetryClientReconnect { json },
        Some(CliCommand::Node {
            command: NodeCommand::List,
        }) => Operation::NodeList { json },
        Some(CliCommand::Node {
            command: NodeCommand::Show { node },
        }) => Operation::NodeShow {
            node: parse_node_id(&node).map_err(str::to_owned)?,
            json,
        },
        Some(CliCommand::Node {
            command: NodeCommand::SetName { name },
        }) => {
            if name.chars().any(char::is_control) {
                return Err("device name cannot contain control characters".into());
            }
            Operation::SetDeviceName {
                name: DisplayName::new(name.trim()).map_err(|_| "invalid device name")?,
                json,
            }
        }
        Some(CliCommand::Peer {
            command: PeerCommand::LocalKey,
        }) => Operation::LocalKey { json },
        Some(CliCommand::Peer {
            command:
                PeerCommand::VerifyLocalKey {
                    network,
                    node,
                    fingerprint,
                },
        }) => Operation::VerifyLocalKey {
            network: network
                .as_deref()
                .map(parse_network_id)
                .transpose()
                .map_err(str::to_owned)?,
            node: parse_node_id(&node).map_err(str::to_owned)?,
            fingerprint: SpkiSha256::parse_canonical_hex(&fingerprint)
                .map_err(|_| "invalid canonical SPKI SHA-256 fingerprint")?
                .to_canonical_hex(),
            json,
        },
        Some(CliCommand::Peer {
            command:
                PeerCommand::SetJoinHost {
                    network,
                    node,
                    fingerprint,
                },
        }) => Operation::SetJoinHost {
            network: parse_network_id(&network).map_err(str::to_owned)?,
            node: parse_node_id(&node).map_err(str::to_owned)?,
            fingerprint: SpkiSha256::parse_canonical_hex(&fingerprint)
                .map_err(|_| "invalid canonical SPKI SHA-256 fingerprint")?
                .to_canonical_hex(),
            json,
        },
        Some(CliCommand::Peer {
            command: PeerCommand::ClearJoinHost,
        }) => Operation::ClearJoinHost { json },
        Some(CliCommand::Peer {
            command: PeerCommand::Trust { node },
        }) => Operation::TrustPeer {
            node: parse_node_id(&node).map_err(str::to_owned)?,
            json,
        },
        Some(CliCommand::Peer {
            command: PeerCommand::Enroll { node, fingerprint },
        }) => Operation::EnrollPeer {
            node: parse_node_id(&node).map_err(str::to_owned)?,
            fingerprint: SpkiSha256::parse_canonical_hex(&fingerprint)
                .map_err(|_| "invalid canonical SPKI SHA-256 fingerprint")?
                .to_canonical_hex(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{format_diagnostics_summary, mutation_ack};

    use blueroute_core::{DisplayName, HealthLevel, NetworkId, NodeCapabilities, NodeId};

    use blueroute_protocol::{ApiVersion, DiagnosticSnapshot, SystemDiagnosticMetadata};

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).into()).collect()
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
            parse(&args(&["network", "link"])),
            Ok(Operation::ClientLink { json: false })
        );
        assert_eq!(
            parse(&args(&["--json", "network", "reconnect"])),
            Ok(Operation::RetryClientReconnect { json: true })
        );
        assert!(parse(&args(&["network", "reconnect", "extra"])).is_err());
        assert_eq!(
            parse(&args(&["node", "set-name", "  Kitchen laptop "])),
            Ok(Operation::SetDeviceName {
                name: DisplayName::new("Kitchen laptop").unwrap(),
                json: false,
            })
        );
        assert!(parse(&args(&["node", "set-name", "bad\u{1b}name"])).is_err());
        assert!(parse(&args(&["node", "set-name", "   "])).is_err());
        assert_eq!(
            parse(&args(&["diagnostics", "show", "--json"])),
            Ok(Operation::Diagnostics {
                json: true,
                summary: false,
            })
        );
        assert!(parse(&args(&["diagnostics"])).is_err());
        assert!(parse(&args(&["network", "join"])).is_err());
        let host_network = NetworkId::from_bytes([0x55; 16]);
        let host_node = NodeId::from_bytes([0x66; 16]);
        let key = "07".repeat(32);
        assert_eq!(
            parse(&args(&[
                "peer",
                "set-join-host",
                &host_network.to_string(),
                &host_node.to_string(),
                &key,
            ])),
            Ok(Operation::SetJoinHost {
                network: host_network,
                node: host_node,
                fingerprint: key.clone(),
                json: false,
            })
        );
        assert_eq!(
            parse(&args(&["--json", "peer", "clear-join-host"])),
            Ok(Operation::ClearJoinHost { json: true })
        );
        assert_eq!(
            parse(&args(&["peer", "enroll", &host_node.to_string(), &key])),
            Ok(Operation::EnrollPeer {
                node: host_node,
                fingerprint: key.clone(),
                json: false,
            })
        );
        assert!(
            parse(&args(&[
                "peer",
                "enroll",
                &host_node.to_string(),
                "not-hex"
            ]))
            .is_err()
        );
        assert!(
            parse(&args(&[
                "peer",
                "enroll",
                &NodeId::from_bytes([0; 16]).to_string(),
                &key
            ]))
            .is_err()
        );
        for bad in [
            ["peer", "set-join-host", "x", "y", "z"]
                .map(str::to_owned)
                .to_vec(),
            args(&[
                "peer",
                "set-join-host",
                &host_network.to_string(),
                &host_node.to_string(),
                "DEADBEEF",
            ]),
            args(&[
                "peer",
                "set-join-host",
                &NetworkId::from_bytes([0; 16]).to_string(),
                &host_node.to_string(),
                &key,
            ]),
            args(&[
                "peer",
                "set-join-host",
                &host_network.to_string(),
                &NodeId::from_bytes([0; 16]).to_string(),
                &key,
            ]),
            args(&[
                "peer",
                "set-join-host",
                &host_network.to_string(),
                &host_node.to_string(),
                &"AB".repeat(32),
            ]),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
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
            authenticated_control_peers: None,
            recent_errors: Vec::new(),
        };
        let unknown = format_diagnostics_summary(&snapshot);
        assert!(unknown.contains("Health: Degraded"));
        assert!(unknown.contains("Interfaces: unavailable (not reported by daemon)"));
        assert!(unknown.contains("Bluetooth adapters: unavailable"));
        assert!(unknown.contains("Observed peers: unavailable"));
        assert!(unknown.contains("this computer plus authenticated peers"));
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
    fn verify_local_key_requires_complete_canonical_operator_tuple() {
        let network = NetworkId::from_bytes([9; 16]);
        let node = NodeId::from_bytes([7; 16]);
        let fingerprint = "ab".repeat(32);
        assert_eq!(
            parse(&args(&[
                "peer",
                "verify-local-key",
                &node.to_string(),
                &fingerprint,
                "--network",
                &network.to_string(),
                "--json",
            ])),
            Ok(Operation::VerifyLocalKey {
                network: Some(network),
                node,
                fingerprint: fingerprint.clone(),
                json: true,
            })
        );
        assert_eq!(
            parse(&args(&[
                "peer",
                "verify-local-key",
                &node.to_string(),
                &fingerprint
            ])),
            Ok(Operation::VerifyLocalKey {
                network: None,
                node,
                fingerprint: fingerprint.clone(),
                json: false,
            })
        );
        for invalid in [
            args(&["peer", "verify-local-key"]),
            args(&["peer", "verify-local-key", &node.to_string()]),
            args(&["peer", "verify-local-key", &node.to_string(), "invalid"]),
            args(&[
                "peer",
                "verify-local-key",
                &node.to_string(),
                &"AB".repeat(32),
            ]),
            args(&[
                "peer",
                "verify-local-key",
                &node.to_string(),
                &fingerprint,
                "--network",
                &NetworkId::from_bytes([0; 16]).to_string(),
            ]),
            args(&[
                "peer",
                "verify-local-key",
                &NodeId::from_bytes([0; 16]).to_string(),
                &fingerprint,
            ]),
        ] {
            assert!(parse(&invalid).is_err());
        }
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
}

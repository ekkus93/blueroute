//! Read-only BlueRoute CLI commands through the shared version-checked daemon
//! client. The clap option layer and mutation subcommands are still pending.

use blueroute_client::BlueRouteClient;
use blueroute_core::NodeCapabilities;
use blueroute_protocol::{Command, DaemonStatus, NetworkSummary, Response};

const HELP: &str = "Usage: blueroute [--json] status\n       blueroute [--json] capability show\n       blueroute [--json] network list\n       blueroute --version\n       blueroute --help";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Help,
    Version,
    Status { json: bool },
    Capabilities { json: bool },
    NetworkList { json: bool },
}

fn parse(args: &[String]) -> Result<Operation, &'static str> {
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        return Ok(Operation::Help);
    }
    if args == ["--version"] || args == ["-V"] {
        return Ok(Operation::Version);
    }
    let json = args.iter().any(|arg| arg == "--json");
    if args.iter().filter(|arg| arg.as_str() == "--json").count() > 1 {
        return Err("duplicate --json option");
    }
    let tokens: Vec<_> = args
        .iter()
        .filter(|arg| arg.as_str() != "--json")
        .map(String::as_str)
        .collect();
    match tokens.as_slice() {
        ["status"] => Ok(Operation::Status { json }),
        ["capability", "show"] => Ok(Operation::Capabilities { json }),
        ["network", "list"] => Ok(Operation::NetworkList { json }),
        _ => Err("unknown command or option"),
    }
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
        Ok(format!("BlueRoute capabilities:\n{caps:#?}"))
    }
}

fn format_networks(
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

fn run(args: &[String]) -> i32 {
    let operation = match parse(args) {
        Ok(operation) => operation,
        Err(message) => {
            eprintln!("blueroute: {message}\n{HELP}");
            return 2;
        }
    };
    match operation {
        Operation::Help => {
            println!("{HELP}");
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
            .map_err(|error| error.to_string())
            .and_then(|status| format_status(&status, json).map_err(|error| error.to_string())),
        Operation::Capabilities { json } => futures_lite::future::block_on(client.capabilities())
            .map_err(|error| error.to_string())
            .and_then(|caps| format_capabilities(&caps, json).map_err(|error| error.to_string())),
        Operation::NetworkList { json } => {
            futures_lite::future::block_on(client.request(&Command::ListNetworks))
                .map_err(|error| error.to_string())
                .and_then(|response| match response {
                    Response::Networks(networks) => {
                        format_networks(&networks, json).map_err(|error| error.to_string())
                    }
                    _ => Err("daemon returned an unexpected network-list response".to_owned()),
                })
        },
        Operation::Help | Operation::Version => unreachable!(),
    };
    match result {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("blueroute: {error}");
            1
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
        assert!(parse(&args(&["network", "join"])).is_err());
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

//! CLI execution against the daemon API. Contains no Bluetooth or networking code: every
//! operation is a typed daemon command (P10-001).

use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use blueroute_client::{BlueRouteClient, ClientError};
use blueroute_core::{DisplayName, NetworkId, NodeId};
use blueroute_protocol::{Command as ApiCommand, Response};

use crate::args::{CapabilityAction, Cli, Command, NetworkAction, NodeAction, TrustAction};
use crate::exit::{Exit, classify};

pub type DaemonFuture<'a> = Pin<Box<dyn Future<Output = Result<Response, ClientError>> + 'a>>;

/// What the CLI needs from the daemon. Implemented by the real D-Bus client and by test fakes.
pub trait Daemon {
    fn request(&self, command: ApiCommand) -> DaemonFuture<'_>;
}

impl Daemon for BlueRouteClient {
    fn request(&self, command: ApiCommand) -> DaemonFuture<'_> {
        Box::pin(async move { BlueRouteClient::request(self, &command).await })
    }
}

pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Run one parsed command and return the process exit status.
pub async fn run(cli: &Cli, daemon: &dyn Daemon, io: &mut Io<'_>) -> Exit {
    let request = match build_request(&cli.command) {
        Ok(request) => request,
        Err(message) => {
            let _ = writeln!(io.err, "blueroute: {message}");
            return Exit::Usage;
        }
    };
    match daemon.request(request).await {
        Ok(response) => {
            crate::render::render(&cli.command, cli.json, &response, io.out);
            Exit::Success
        }
        Err(error) => {
            report_error(&error, io.err);
            classify(&error)
        }
    }
}

/// Translate a parsed command into exactly one daemon request. Argument values are validated
/// here so a typo never reaches the daemon.
pub fn build_request(command: &Command) -> Result<ApiCommand, String> {
    Ok(match command {
        Command::Status => ApiCommand::GetStatus,
        Command::Capability {
            action: CapabilityAction::Show,
        } => ApiCommand::GetCapabilities,
        Command::Node {
            action: NodeAction::List,
        } => ApiCommand::ListNodes,
        Command::Node {
            action: NodeAction::Show { id },
        } => ApiCommand::GetNode {
            node: parse_id::<NodeId>("node", id)?,
        },
        Command::Network { action } => match action {
            NetworkAction::List => ApiCommand::ListNetworks,
            NetworkAction::Create { name } => ApiCommand::CreateNetwork {
                name: DisplayName::new(name.as_str())
                    .map_err(|e| format!("invalid network name: {}", e.message()))?,
            },
            NetworkAction::Join { id } => ApiCommand::JoinNetwork {
                network: parse_id::<NetworkId>("network", id)?,
            },
            NetworkAction::Leave => ApiCommand::LeaveNetwork,
            NetworkAction::Discover { stop: false } => ApiCommand::StartDiscovery,
            NetworkAction::Discover { stop: true } => ApiCommand::StopDiscovery,
        },
        Command::Trust { action } => match action {
            TrustAction::Approve { id } => ApiCommand::TrustPeer {
                node: parse_id("node", id)?,
            },
            TrustAction::Forget { id } => ApiCommand::ForgetPeer {
                node: parse_id("node", id)?,
            },
        },
        Command::Diagnostics(_) => ApiCommand::GetDiagnostics,
    })
}

fn parse_id<T: std::str::FromStr>(what: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("`{value}` is not a valid {what} id (expected 32 hex characters)"))
}

pub fn report_error(error: &ClientError, err: &mut dyn Write) {
    let exit = classify(error);
    let hint = match exit {
        Exit::DaemonUnavailable => {
            "\nIs the BlueRoute daemon running? Try: systemctl status blueroute"
        }
        Exit::Unauthorized => "\nYou may need to authenticate or run this as an administrator.",
        _ => "",
    };
    let _ = writeln!(err, "blueroute: {error}{hint}");
}

#[cfg(test)]
pub mod testing {
    use std::cell::RefCell;

    use super::*;

    /// Scripted daemon: returns queued results and records every command it receives.
    pub struct FakeDaemon {
        pub replies: RefCell<Vec<Result<Response, ClientError>>>,
        pub seen: RefCell<Vec<ApiCommand>>,
    }

    impl FakeDaemon {
        pub fn replying(replies: Vec<Result<Response, ClientError>>) -> Self {
            Self {
                replies: RefCell::new(replies.into_iter().rev().collect()),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Daemon for FakeDaemon {
        fn request(&self, command: ApiCommand) -> DaemonFuture<'_> {
            self.seen.borrow_mut().push(command);
            let reply = self
                .replies
                .borrow_mut()
                .pop()
                .expect("unexpected extra request");
            Box::pin(async move { reply })
        }
    }

    pub fn run_args(args: &[&str], daemon: &FakeDaemon) -> (Exit, String, String) {
        use clap::Parser;
        let cli = Cli::try_parse_from(args).expect("test arguments parse");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = futures_lite::future::block_on(run(
            &cli,
            daemon,
            &mut Io {
                out: &mut out,
                err: &mut err,
            },
        ));
        (
            exit,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use blueroute_core::{HealthLevel, NodeCapabilities};
    use blueroute_protocol::{ApiVersion, DaemonStatus};
    use zbus::fdo;

    use super::testing::*;
    use super::*;

    fn status() -> Response {
        Response::Status(DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: None,
            current_network: None,
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        })
    }

    #[test]
    fn status_succeeds_in_text_and_json() {
        let daemon = FakeDaemon::replying(vec![Ok(status()), Ok(status())]);
        let (exit, out, _) = run_args(&["blueroute", "status"], &daemon);
        assert_eq!(exit, Exit::Success);
        assert!(out.contains("working normally") && out.contains("not in a network"));
        let (_, json, _) = run_args(&["blueroute", "--json", "status"], &daemon);
        let value: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(value["type"], "status");
        assert_eq!(
            daemon.seen.borrow().as_slice(),
            [ApiCommand::GetStatus, ApiCommand::GetStatus]
        );
    }

    #[test]
    fn daemon_errors_set_the_documented_exit_status_and_explain() {
        let cases = [
            (fdo::Error::AccessDenied("no".into()), Exit::Unauthorized),
            (
                fdo::Error::NotSupported("no adapter".into()),
                Exit::Unsupported,
            ),
            (
                fdo::Error::ServiceUnknown("gone".into()),
                Exit::DaemonUnavailable,
            ),
            (fdo::Error::Failed("boom".into()), Exit::OperationFailed),
        ];
        for (error, expected) in cases {
            let daemon = FakeDaemon::replying(vec![Err(ClientError::Bus(zbus::Error::FDO(
                Box::new(error),
            )))]);
            let (exit, out, err) = run_args(&["blueroute", "status"], &daemon);
            assert_eq!(exit, expected);
            assert!(out.is_empty());
            assert!(err.starts_with("blueroute: "), "{err}");
        }
    }

    fn node_id(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn net_id(b: u8) -> NetworkId {
        NetworkId::from_bytes([b; 16])
    }

    fn parse(args: &[&str]) -> Cli {
        use clap::Parser;
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn every_command_maps_to_exactly_one_daemon_request() {
        let node = node_id(1).to_string();
        let net = net_id(2).to_string();
        let cases: Vec<(Vec<&str>, ApiCommand)> = vec![
            (vec!["blueroute", "status"], ApiCommand::GetStatus),
            (
                vec!["blueroute", "capability", "show"],
                ApiCommand::GetCapabilities,
            ),
            (vec!["blueroute", "node", "list"], ApiCommand::ListNodes),
            (
                vec!["blueroute", "node", "show", &node],
                ApiCommand::GetNode { node: node_id(1) },
            ),
            (
                vec!["blueroute", "network", "list"],
                ApiCommand::ListNetworks,
            ),
            (
                vec!["blueroute", "network", "create", "Home"],
                ApiCommand::CreateNetwork {
                    name: DisplayName::new("Home").unwrap(),
                },
            ),
            (
                vec!["blueroute", "network", "join", &net],
                ApiCommand::JoinNetwork { network: net_id(2) },
            ),
            (
                vec!["blueroute", "network", "leave"],
                ApiCommand::LeaveNetwork,
            ),
            (
                vec!["blueroute", "network", "discover"],
                ApiCommand::StartDiscovery,
            ),
            (
                vec!["blueroute", "network", "discover", "--stop"],
                ApiCommand::StopDiscovery,
            ),
            (
                vec!["blueroute", "trust", "approve", &node],
                ApiCommand::TrustPeer { node: node_id(1) },
            ),
            (
                vec!["blueroute", "trust", "forget", &node],
                ApiCommand::ForgetPeer { node: node_id(1) },
            ),
            (vec!["blueroute", "diagnostics"], ApiCommand::GetDiagnostics),
        ];
        for (argv, expected) in cases {
            assert_eq!(
                build_request(&parse(&argv).command),
                Ok(expected),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn invalid_ids_and_names_fail_before_any_daemon_call() {
        for argv in [
            vec!["blueroute", "node", "show", "nope"],
            vec!["blueroute", "network", "join", "1234"],
            vec!["blueroute", "trust", "approve", "zz"],
            vec!["blueroute", "network", "create", ""],
        ] {
            let daemon = FakeDaemon::replying(vec![]);
            let (exit, out, err) = run_args(&argv, &daemon);
            assert_eq!(exit, Exit::Usage, "{argv:?}");
            assert!(out.is_empty() && err.starts_with("blueroute: "));
            assert!(
                daemon.seen.borrow().is_empty(),
                "{argv:?} reached the daemon"
            );
        }
    }

    #[test]
    fn nothing_is_approved_unless_the_user_names_a_computer() {
        // `trust` with no subcommand or id is a usage error at parse time.
        use clap::Parser;
        assert!(Cli::try_parse_from(["blueroute", "trust"]).is_err());
        assert!(Cli::try_parse_from(["blueroute", "trust", "approve"]).is_err());
        // A denied approval surfaces as an authorization failure, not silent success.
        let daemon = FakeDaemon::replying(vec![Err(ClientError::Bus(zbus::Error::FDO(Box::new(
            fdo::Error::AccessDenied("not allowed".into()),
        ))))]);
        let (exit, out, _) = run_args(
            &["blueroute", "trust", "approve", &node_id(1).to_string()],
            &daemon,
        );
        assert_eq!((exit, out.is_empty()), (Exit::Unauthorized, true));
    }

    #[test]
    fn lifecycle_commands_report_done_and_stay_quiet_in_json_ack() {
        let daemon = FakeDaemon::replying(vec![Ok(Response::Ack), Ok(Response::Ack)]);
        let (exit, out, _) = run_args(&["blueroute", "network", "leave"], &daemon);
        assert_eq!((exit, out.as_str()), (Exit::Success, "Done.\n"));
        let (_, json, _) = run_args(&["blueroute", "--json", "network", "leave"], &daemon);
        assert_eq!(json.trim(), r#"{"type":"ack"}"#);
    }

    #[test]
    fn node_and_network_lists_are_friendly_and_json_is_stable() {
        use blueroute_core::Reachability;
        use blueroute_protocol::{NetworkSummary, NodeSummary};
        let nodes = vec![NodeSummary {
            id: node_id(7),
            name: DisplayName::new("Kitchen laptop").unwrap(),
            reachability: Reachability::Routed,
        }];
        let networks = vec![NetworkSummary {
            id: net_id(9),
            name: DisplayName::new("Home").unwrap(),
            member_count: 1,
        }];
        let daemon = FakeDaemon::replying(vec![
            Ok(Response::Nodes(nodes.clone())),
            Ok(Response::Nodes(nodes)),
            Ok(Response::Networks(networks.clone())),
            Ok(Response::Networks(networks)),
            Ok(Response::Networks(vec![])),
            Ok(Response::Node(None)),
        ]);
        let (_, text, _) = run_args(&["blueroute", "node", "list"], &daemon);
        assert!(text.contains("Kitchen laptop") && text.contains("through another computer"));
        assert!(!text.contains("Routed") && !text.contains("PANU"));
        let (_, json, _) = run_args(&["blueroute", "--json", "node", "list"], &daemon);
        assert_eq!(
            json.trim(),
            format!(
                r#"{{"type":"nodes","data":[{{"id":"{}","name":"Kitchen laptop","reachability":"routed"}}]}}"#,
                node_id(7)
            )
        );
        let (_, text, _) = run_args(&["blueroute", "network", "list"], &daemon);
        assert!(text.contains("Home") && text.contains("1 member\n"));
        let (_, json, _) = run_args(&["blueroute", "--json", "network", "list"], &daemon);
        assert_eq!(
            json.trim(),
            format!(
                r#"{{"type":"networks","data":[{{"id":"{}","name":"Home","member_count":1}}]}}"#,
                net_id(9)
            )
        );
        let (_, text, _) = run_args(&["blueroute", "network", "list"], &daemon);
        assert!(text.contains("discover"));
        let (_, text, _) = run_args(
            &["blueroute", "node", "show", &node_id(1).to_string()],
            &daemon,
        );
        assert!(text.contains("No such computer"));
    }

    #[test]
    fn capability_output_keeps_unknown_distinct_from_no() {
        use blueroute_core::{CapabilitySource, Sourced};
        let caps = NodeCapabilities {
            panu: Some(Sourced::new(true, CapabilitySource::Measured)),
            nap: Some(Sourced::new(false, CapabilitySource::Discovered)),
            ..NodeCapabilities::default()
        };
        let daemon = FakeDaemon::replying(vec![
            Ok(Response::Capabilities(caps.clone())),
            Ok(Response::Capabilities(caps)),
        ]);
        let (_, text, _) = run_args(&["blueroute", "capability", "show"], &daemon);
        assert!(text.contains("Can join a network: yes (Measured)"));
        assert!(text.contains("Can host a network: no (Discovered)"));
        assert!(text.contains("Can pass traffic between networks: unknown"));
        let (_, json, _) = run_args(&["blueroute", "--json", "capability", "show"], &daemon);
        let value: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(value["data"]["panu"]["value"], true);
        assert!(value["data"]["routing"].is_null());
    }

    #[test]
    fn diagnostics_is_concise_by_default_detailed_on_request_and_json_capable() {
        use blueroute_core::{CapabilitySource, NetworkBackend, Sourced};
        let snapshot = blueroute_protocol::DiagnosticSnapshot {
            api_version: ApiVersion::CURRENT,
            health: HealthLevel::Degraded,
            current_network: Some(net_id(3)),
            visible_nodes: 2,
            capabilities: NodeCapabilities {
                nap: Some(Sourced::new(false, CapabilitySource::Discovered)),
                network_backend: Some(Sourced::new(
                    NetworkBackend::NetworkManager,
                    CapabilitySource::Discovered,
                )),
                ..NodeCapabilities::default()
            },
        };
        let daemon = FakeDaemon::replying(vec![
            Ok(Response::Diagnostics(snapshot.clone())),
            Ok(Response::Diagnostics(snapshot.clone())),
            Ok(Response::Diagnostics(snapshot)),
        ]);
        let (_, concise, _) = run_args(&["blueroute", "diagnostics"], &daemon);
        assert!(
            concise.contains("working, with limits") && concise.contains("Computers in view: 2")
        );
        assert!(concise.contains("Not available here: Can host a network"));
        assert!(!concise.contains("Daemon API version"));
        let (_, detailed, _) = run_args(&["blueroute", "diagnostics", "--detailed"], &daemon);
        assert!(detailed.contains("Daemon API version: 1.0"));
        assert!(detailed.contains("Network manager: NetworkManager (Discovered)"));
        let (_, json, _) = run_args(&["blueroute", "--json", "diagnostics"], &daemon);
        let value: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(value["data"]["visible_nodes"], 2);
        for text in [&concise, &detailed, &json] {
            assert!(!text.to_lowercase().contains("secret") && !text.contains("key"));
        }
    }

    #[test]
    fn unavailable_daemon_message_suggests_the_next_step() {
        let daemon = FakeDaemon::replying(vec![Err(ClientError::Bus(zbus::Error::FDO(Box::new(
            fdo::Error::ServiceUnknown("x".into()),
        ))))]);
        let (_, _, err) = run_args(&["blueroute", "status"], &daemon);
        assert!(err.contains("daemon running"));
    }
}

//! Running one parsed command against the daemon and printing its result.

use super::args::{Cli, Operation, parse};
use super::error_guidance;
use super::failure::CliFailure;
use super::format::{
    format_capabilities, format_client_link, format_diagnostics, format_diagnostics_summary,
    format_networks, format_node, format_nodes, format_status, mutation_ack,
};
use super::local_key::{format_local_key, require_stable_local_identity, verify_local_key};
use blueroute_client::BlueRouteClient;
use blueroute_protocol::{Command, Response};
use clap::CommandFactory;

pub(super) fn run(args: &[String]) -> i32 {
    // Keep the machine-readable stderr contract even for parse failures.
    let json_mode = args.iter().any(|argument| argument == "--json");
    let operation = match parse(args) {
        Ok(operation) => operation,
        Err(message) => {
            eprintln!("{}", error_guidance::format_failure(2, &message, json_mode));
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
            // An initial bus/version connection failure is always unavailable.
            // Preserve exit 3 even if zbus internally classifies the transport error
            // as a generic operational failure.
            eprintln!(
                "{}",
                error_guidance::format_failure(
                    3,
                    &format!("daemon unavailable/incompatible: {error}"),
                    json_mode,
                )
            );
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
                        Response::Diagnostics(snapshot) => {
                            let after = futures_lite::future::block_on(client.status())
                                .map_err(CliFailure::from)?;
                            require_stable_local_identity(&status, &snapshot, &after)
                                .map_err(CliFailure::from)?;
                            format_local_key(
                                status.local_node,
                                status.current_network,
                                snapshot.local_control_tls_spki_sha256.as_deref(),
                                json,
                            )
                            .map_err(CliFailure::from)
                        }
                        _ => Err("daemon returned an unexpected local-key response".into()),
                    })
            }),
        Operation::VerifyLocalKey {
            network,
            node,
            fingerprint,
            json,
        } => futures_lite::future::block_on(client.status())
            .map_err(CliFailure::from)
            .and_then(|status| {
                futures_lite::future::block_on(client.request(&Command::GetDiagnostics))
                    .map_err(CliFailure::from)
                    .and_then(|response| match response {
                        Response::Diagnostics(snapshot) => {
                            let after = futures_lite::future::block_on(client.status())
                                .map_err(CliFailure::from)?;
                            require_stable_local_identity(&status, &snapshot, &after)
                                .map_err(CliFailure::from)?;
                            verify_local_key(&status, &snapshot, network, node, &fingerprint, json)
                                .map_err(CliFailure::from)
                        }
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
        Operation::SetDeviceName { name, json } => {
            futures_lite::future::block_on(client.request(&Command::SetDeviceName { name }))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Device name changed")),
                    _ => Err("daemon returned an unexpected rename response".into()),
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
        Operation::EnrollPeer {
            node,
            fingerprint,
            json,
        } => futures_lite::future::block_on(
            client.request(&Command::EnrollPeerKey { node, fingerprint }),
        )
        .map_err(CliFailure::from)
        .and_then(|response| match response {
            Response::Ack => Ok(mutation_ack(json, "Peer key enrolled")),
            _ => Err("daemon returned an unexpected enrollment response".into()),
        }),
        Operation::SetJoinHost {
            network,
            node,
            fingerprint,
            json,
        } => futures_lite::future::block_on(client.request(&Command::RecordJoinHost {
            network,
            node,
            fingerprint,
        }))
        .map_err(CliFailure::from)
        .and_then(|response| match response {
            Response::Ack => Ok(mutation_ack(json, "Join host recorded")),
            _ => Err("daemon returned an unexpected join-host response".into()),
        }),
        Operation::ClearJoinHost { json } => {
            futures_lite::future::block_on(client.request(&Command::ClearJoinHost))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Join host cleared")),
                    _ => Err("daemon returned an unexpected join-host response".into()),
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
        Operation::ClientLink { json } => {
            futures_lite::future::block_on(client.request(&Command::GetClientLink))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::ClientLink(link) => {
                        format_client_link(link.as_ref(), json).map_err(CliFailure::from)
                    }
                    _ => Err("daemon returned an unexpected client-link response".into()),
                })
        }
        Operation::RetryClientReconnect { json } => {
            futures_lite::future::block_on(client.request(&Command::RetryClientReconnect))
                .map_err(CliFailure::from)
                .and_then(|response| match response {
                    Response::Ack => Ok(mutation_ack(json, "Reconnecting to the host")),
                    _ => Err("daemon returned an unexpected reconnect response".into()),
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
            eprintln!(
                "{}",
                error_guidance::format_failure_with(
                    error.code,
                    error.detail,
                    &error.message,
                    json_mode
                )
            );
            error.code
        }
    }
}

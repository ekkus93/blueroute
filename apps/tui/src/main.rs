//! Read-only terminal views backed by the version-checked daemon client.
//! A full Ratatui event loop and event subscription remain separate P11 work.
#[cfg(test)]
use std::io::BufRead;
use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use blueroute_client::{BlueRouteClient, ClientErrorKind};
use blueroute_core::HealthLevel;
use blueroute_protocol::{
    Command, DaemonStatus, DiagnosticSnapshot, NetworkSummary, NodeSummary, Response,
};
use futures_lite::future;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Overview,
    Networks,
    Devices,
    Diagnostics,
}

impl Screen {
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Networks => "Networks",
            Self::Devices => "Devices",
            Self::Diagnostics => "Diagnostics",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Show(Screen),
    Refresh,
    Help,
    Quit,
    Invalid,
}

fn parse_action(input: &str) -> Action {
    match input.trim().to_ascii_lowercase().as_str() {
        "1" | "o" => Action::Show(Screen::Overview),
        "2" | "n" => Action::Show(Screen::Networks),
        "3" | "d" => Action::Show(Screen::Devices),
        "4" | "i" => Action::Show(Screen::Diagnostics),
        "r" | "refresh" => Action::Refresh,
        "h" | "?" => Action::Help,
        "q" | "quit" | "exit" => Action::Quit,
        _ => Action::Invalid,
    }
}

fn render(screen: Screen, body: &str) -> String {
    format!(
        "BlueRoute | {}\n\n{body}\n\n1 Overview   2 Networks   3 Devices   4 Diagnostics\nr Refresh    h Help       q Quit\n",
        screen.title()
    )
}

fn health_label(health: HealthLevel) -> &'static str {
    match health {
        HealthLevel::Healthy => "Healthy (local daemon only)",
        HealthLevel::Degraded => "Degraded",
        HealthLevel::Reconnecting => "Reconnecting",
        HealthLevel::Error => "Error",
    }
}

fn support_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "available locally",
        Some(false) => "unavailable locally",
        None => "unknown",
    }
}

fn format_overview(status: &DaemonStatus) -> String {
    let adapter = status.capabilities.adapter_usable.as_ref().map(|v| v.value);
    let warning = match adapter {
        Some(true) => "Adapter: available (not proof of peer connectivity)",
        Some(false) => "Warning: Bluetooth adapter unavailable",
        None => "Warning: Bluetooth adapter availability unknown",
    };
    format!(
        "Status: {}\nLocal node: {}\nCurrent network: {}\n{}\nCan host: {}\nCan join: {}\nRemote peer count: unavailable (not authenticated yet)\n\nNo Internet sharing or authenticated joining is enabled.",
        health_label(status.health),
        status
            .local_node
            .map_or_else(|| "unavailable".to_owned(), |id| id.to_string()),
        status
            .current_network
            .map_or_else(|| "none reported".to_owned(), |id| id.to_string()),
        warning,
        support_label(status.capabilities.can_host_pan()),
        support_label(status.capabilities.can_join_pan()),
    )
}

fn format_networks(networks: &[NetworkSummary]) -> String {
    let mut lines = vec!["Daemon-reported networks (discovery is not authentication):".to_owned()];
    if networks.is_empty() {
        lines.push("No networks reported.".to_owned());
    } else {
        for network in networks.iter().take(32) {
            lines.push(format!(
                "  {} — {} ({} recorded members; not live peers)",
                network.name, network.id, network.member_count
            ));
        }
        if networks.len() > 32 {
            lines.push(format!(
                "  ... {} additional entries omitted",
                networks.len() - 32
            ));
        }
    }
    lines.push("Creating, joining and leaving networks is not available in this view.".to_owned());
    lines.join("\n")
}

fn format_devices(nodes: &[NodeSummary]) -> String {
    let mut lines =
        vec!["Daemon-reported nodes (remote reachability is not yet authenticated):".to_owned()];
    if nodes.is_empty() {
        lines.push("No nodes reported.".to_owned());
    } else {
        for node in nodes.iter().take(32) {
            lines.push(format!(
                "  {} — {} ({:?})",
                node.name, node.id, node.reachability
            ));
        }
        if nodes.len() > 32 {
            lines.push(format!(
                "  ... {} additional entries omitted",
                nodes.len() - 32
            ));
        }
    }
    lines.push("Peer trust is not TLS key enrollment or membership.".to_owned());
    lines.join("\n")
}

fn observed_count<T>(values: Option<&[T]>) -> String {
    match values {
        Some(values) => format!("{} reported", values.len()),
        None => "unavailable (not observed)".to_owned(),
    }
}

fn format_diagnostics(snapshot: &DiagnosticSnapshot) -> String {
    let (adapters, bluez, backend) = match snapshot.system.as_ref() {
        Some(system) => (
            format!("{} observed", system.adapters.len()),
            if system.bluez_version.is_some() {
                "reported"
            } else {
                "unavailable"
            },
            if system.network_backend_version.is_some() {
                "reported"
            } else {
                "unavailable"
            },
        ),
        None => ("unavailable".to_owned(), "unavailable", "unavailable"),
    };
    format!(
        "Local daemon health: {}\nAdapters: {adapters}\nBlueZ version: {bluez}\nNetwork backend version: {backend}\nObserved peers: {}\nInterfaces: {}\nRoutes: {}\nTopology: {}\nRecent API failures: {}\n\nThese are daemon observations, not Bluetooth data-plane acceptance.",
        health_label(snapshot.health),
        observed_count(snapshot.observed_peers.as_deref()),
        observed_count(snapshot.interfaces.as_deref()),
        observed_count(snapshot.routes.as_deref()),
        if snapshot.topology.is_some() {
            "reported"
        } else {
            "unavailable (not observed)"
        },
        snapshot.recent_errors.len(),
    )
}

fn offline_message(kind: ClientErrorKind) -> &'static str {
    match kind {
        ClientErrorKind::Unavailable => {
            "Daemon unavailable or incompatible. Check blueroute.service and press r to retry."
        }
        ClientErrorKind::Authorization => {
            "Read-only access denied by system policy. Check D-Bus permissions."
        }
        ClientErrorKind::Unsupported => "Daemon does not support this query.",
        ClientErrorKind::Operational => {
            "Daemon query failed. Use blueroute diagnostics show for details."
        }
    }
}

async fn fetch_view(screen: Screen) -> String {
    // Open a fresh version-checked client on each navigation/refresh. This
    // recovers after daemon restarts without replaying any privileged action.
    let client = match BlueRouteClient::connect().await {
        Ok(client) => client,
        Err(error) => return offline_message(error.kind()).to_owned(),
    };
    match screen {
        Screen::Overview => match client.status().await {
            Ok(status) => format_overview(&status),
            Err(error) => offline_message(error.kind()).to_owned(),
        },
        Screen::Networks => match client.request(&Command::ListNetworks).await {
            Ok(Response::Networks(networks)) => format_networks(&networks),
            Ok(_) => "Daemon returned an unexpected network response.".to_owned(),
            Err(error) => offline_message(error.kind()).to_owned(),
        },
        Screen::Devices => match client.request(&Command::ListNodes).await {
            Ok(Response::Nodes(nodes)) => format_devices(&nodes),
            Ok(_) => "Daemon returned an unexpected node response.".to_owned(),
            Err(error) => offline_message(error.kind()).to_owned(),
        },
        Screen::Diagnostics => match client.request(&Command::GetDiagnostics).await {
            Ok(Response::Diagnostics(snapshot)) => format_diagnostics(&snapshot),
            Ok(_) => "Daemon returned an unexpected diagnostic response.".to_owned(),
            Err(error) => offline_message(error.kind()).to_owned(),
        },
    }
}

#[cfg(test)]
fn run_with_source<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    mut load: impl FnMut(Screen) -> String,
) -> io::Result<()> {
    let mut screen = Screen::Overview;
    writeln!(output, "{}", render(screen, &load(screen)))?;
    loop {
        write!(output, "> ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        match parse_action(&line) {
            Action::Quit => break,
            Action::Show(next) => {
                screen = next;
                writeln!(output, "{}", render(screen, &load(screen)))?;
            }
            Action::Refresh => writeln!(output, "{}", render(screen, &load(screen)))?,
            Action::Help => writeln!(output, "Select 1-4 for a view, r to refresh, q to quit.")?,
            Action::Invalid => writeln!(output, "Unknown command. Press h for help.")?,
        }
    }
    Ok(())
}

// A background reader keeps the terminal responsive to periodic daemon checks
// even when the operator has not entered a command. No privileged operation is
// ever issued by this loop.
fn run_live<W: Write>(
    output: &mut W,
    input: &Receiver<String>,
    interval: Duration,
    mut load: impl FnMut(Screen) -> String,
) -> io::Result<()> {
    let mut screen = Screen::Overview;
    let mut body = load(screen);
    writeln!(output, "{}", render(screen, &body))?;
    output.flush()?;
    loop {
        match input.recv_timeout(interval) {
            Ok(line) => match parse_action(&line) {
                Action::Quit => return Ok(()),
                Action::Show(next) => {
                    screen = next;
                    body = load(screen);
                    writeln!(output, "{}", render(screen, &body))?;
                }
                Action::Refresh => {
                    body = load(screen);
                    writeln!(output, "{}", render(screen, &body))?;
                }
                Action::Help => {
                    writeln!(output, "Select 1-4 for a view, r to refresh, q to quit.")?
                }
                Action::Invalid => writeln!(output, "Unknown command. Press h for help.")?,
            },
            Err(RecvTimeoutError::Timeout) => {
                let refreshed = load(screen);
                if refreshed != body {
                    body = refreshed;
                    writeln!(output, "{}", render(screen, &body))?;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        output.flush()?;
    }
}

fn main() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("BlueRoute TUI requires an interactive text terminal.");
        std::process::exit(2);
    }
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) if sender.send(line).is_ok() => {}
                _ => break,
            }
        }
    });
    run_live(
        &mut io::stdout().lock(),
        &receiver,
        Duration::from_secs(5),
        |screen| future::block_on(fetch_view(screen)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::{DisplayName, NetworkId, NodeCapabilities, NodeId, Reachability};
    use blueroute_protocol::ApiVersion;

    fn local_status() -> DaemonStatus {
        DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([7; 16])),
            current_network: Some(NetworkId::from_bytes([8; 16])),
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        }
    }

    #[test]
    fn navigation_is_case_insensitive_refreshable_and_read_only() {
        assert_eq!(parse_action(" N\n"), Action::Show(Screen::Networks));
        assert_eq!(parse_action("4"), Action::Show(Screen::Diagnostics));
        assert_eq!(parse_action("R"), Action::Refresh);
        assert_eq!(parse_action("quit"), Action::Quit);
        assert_eq!(parse_action("network create"), Action::Invalid);
    }

    #[test]
    fn daemon_refresh_replaces_offline_view_without_restarting_shell() {
        let mut output = Vec::new();
        let mut attempts = 0;
        run_with_source(&mut &b"r\n2\n3\nh\nq\n"[..], &mut output, |_| {
            attempts += 1;
            if attempts == 1 {
                "Daemon unavailable".into()
            } else {
                "Daemon restored".into()
            }
        })
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(attempts, 4);
        assert!(text.contains("Daemon unavailable"));
        assert!(text.contains("BlueRoute | Networks"));
        assert!(text.contains("BlueRoute | Devices"));
        assert!(text.contains("Daemon restored"));
        assert!(text.contains("r to refresh"));
        assert!(!text.contains("BlueRoute | Diagnostics"));
    }

    #[test]
    fn live_shell_reloads_after_timeout_and_reconnects_without_mutations() {
        let (tx, rx) = mpsc::channel();
        let mut output = Vec::new();
        let mut calls = 0;
        run_live(&mut output, &rx, Duration::from_millis(1), |_| {
            calls += 1;
            if calls == 1 {
                "Daemon unavailable".to_owned()
            } else {
                tx.send("q".to_owned()).unwrap();
                "Daemon restored".to_owned()
            }
        })
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(calls, 2);
        assert!(text.contains("Daemon unavailable"));
        assert!(text.contains("Daemon restored"));
    }

    #[test]
    fn live_shell_navigation_and_eof_are_bounded() {
        let (tx, rx) = mpsc::channel();
        tx.send("2".to_owned()).unwrap();
        tx.send("q".to_owned()).unwrap();
        let mut output = Vec::new();
        run_live(&mut output, &rx, Duration::from_secs(1), |screen| {
            screen.title().to_owned()
        })
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("BlueRoute | Networks"));
        assert!(!text.contains("BlueRoute | Devices"));
        drop(tx);
        let mut eof_output = Vec::new();
        run_live(&mut eof_output, &rx, Duration::from_secs(1), |_| {
            "offline".into()
        })
        .unwrap();
    }

    #[test]
    fn overview_distinguishes_local_health_from_authenticated_peer_state() {
        let text = format_overview(&local_status());
        assert!(text.contains("Healthy (local daemon only)"));
        assert!(text.contains("Remote peer count: unavailable"));
        assert!(text.contains("adapter availability unknown"));
        assert!(text.contains("No Internet sharing or authenticated joining is enabled"));
    }

    #[test]
    fn network_and_node_views_use_only_explicit_daemon_records() {
        let id = NetworkId::from_bytes([9; 16]);
        let networks = vec![NetworkSummary {
            id,
            name: DisplayName::new("Local Lab").unwrap(),
            member_count: 2,
        }];
        let text = format_networks(&networks);
        assert!(text.contains("Local Lab"));
        assert!(text.contains("not live peers"));
        assert!(format_networks(&[]).contains("No networks reported"));
        let nodes = vec![NodeSummary {
            id: NodeId::from_bytes([3; 16]),
            name: DisplayName::new("This computer").unwrap(),
            reachability: Reachability::Local,
        }];
        assert!(format_devices(&nodes).contains("This computer"));
        assert!(format_devices(&nodes).contains("not yet authenticated"));
        assert!(format_devices(&[]).contains("No nodes reported"));
    }

    #[test]
    fn unavailable_data_is_not_reported_as_observed_empty() {
        assert_eq!(observed_count::<u8>(None), "unavailable (not observed)");
        assert_eq!(observed_count(Some(&[] as &[u8])), "0 reported");
        assert!(offline_message(ClientErrorKind::Unavailable).contains("press r to retry"));
    }
}

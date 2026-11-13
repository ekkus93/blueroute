//! Full-screen, read-only terminal UI over the version-checked daemon client (P11).
//!
//! Three threads: the main thread owns the terminal and handles keys; a loader thread fetches
//! views so a slow daemon never freezes input; a watcher thread holds a daemon event
//! subscription (with bounded reconnect backoff) and only reports that something changed. The
//! UI then re-reads the typed snapshot. No mutation is ever sent.
use std::io::{self, IsTerminal};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

mod app;
mod diagnostics;
mod watch;

use blueroute_client::{BlueRouteClient, ClientErrorKind};
use blueroute_core::HealthLevel;
use blueroute_protocol::{Command, DaemonStatus, NetworkSummary, NodeSummary, Response};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Screen {
    Overview,
    Networks,
    Devices,
    Diagnostics,
}

impl Screen {
    pub(crate) const ALL: [Self; 4] = [
        Self::Overview,
        Self::Networks,
        Self::Devices,
        Self::Diagnostics,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Networks => "Networks",
            Self::Devices => "Devices",
            Self::Diagnostics => "Diagnostics",
        }
    }
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

/// `peers` is the host's authenticated control peers from diagnostics: `None` when not reported.
fn format_overview(status: &DaemonStatus, peers: Option<&[blueroute_core::NodeId]>) -> String {
    let adapter = status.capabilities.adapter_usable.as_ref().map(|v| v.value);
    let warning = match adapter {
        Some(true) => "Adapter: available (not proof of peer connectivity)",
        Some(false) => "Warning: Bluetooth adapter unavailable",
        None => "Warning: Bluetooth adapter availability unknown",
    };
    format!(
        "Status: {}\nLocal node: {}\nCurrent network: {}\n{}\nCan host: {}\nCan join: {}\nConnected devices: {}",
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
        match peers {
            Some(peers) => format!(
                "{} (authenticated control sessions on this host)",
                peers.len()
            ),
            None => "not reported".to_owned(),
        },
    )
}

/// Actions need a PolicyKit prompt, which a full-screen terminal cannot host, so the TUI shows
/// the exact command to run in a normal shell instead (maintainer decision, 2026-11-12).
const ACTIONS_NOTE: &str =
    "Actions: run these in a normal shell, where you can be asked for your password:";

fn format_networks(
    networks: &[NetworkSummary],
    current: Option<blueroute_core::NetworkId>,
) -> String {
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
    lines.push(String::new());
    lines.push(ACTIONS_NOTE.to_owned());
    match current {
        Some(network) => {
            lines.push(format!(
                "  Leave network {network}:  blueroute network leave"
            ));
            lines.push("  Joined client link:       blueroute network link".to_owned());
        }
        None => {
            lines.push("  Start a network:  blueroute network create NAME".to_owned());
            lines.push(
                "  Join a network:   not available yet (it needs authenticated join)".to_owned(),
            );
        }
    }
    lines.join("\n")
}

fn format_devices(nodes: &[NodeSummary]) -> String {
    let mut lines = vec!["This computer and peers with an authenticated session to it:".to_owned()];
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
    lines.push(String::new());
    lines.push(ACTIONS_NOTE.to_owned());
    lines.push("  This computer's identity:   blueroute peer local-key".to_owned());
    lines
        .push("  Admit a joining computer:   blueroute peer enroll NODE_ID FINGERPRINT".to_owned());
    lines.push("    (read both on that computer with `blueroute peer local-key`)".to_owned());
    lines.push("  Remove a computer:          blueroute peer forget NODE_ID".to_owned());
    lines.join("\n")
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
        ClientErrorKind::AddressConflict => {
            "This network's address range is already used on this computer (see `ip -4 route`)."
        }
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
            Ok(status) => {
                // The count is optional: a failed diagnostics read just leaves it unreported.
                let peers = match client.request(&Command::GetDiagnostics).await {
                    Ok(Response::Diagnostics(snapshot)) => snapshot.authenticated_control_peers,
                    _ => None,
                };
                format_overview(&status, peers.as_deref())
            }
            Err(error) => offline_message(error.kind()).to_owned(),
        },
        Screen::Networks => match client.request(&Command::ListNetworks).await {
            Ok(Response::Networks(networks)) => {
                // Without status the action hints fall back to the not-in-a-network case.
                let current = client
                    .status()
                    .await
                    .ok()
                    .and_then(|status| status.current_network);
                format_networks(&networks, current)
            }
            Ok(_) => "Daemon returned an unexpected network response.".to_owned(),
            Err(error) => offline_message(error.kind()).to_owned(),
        },
        Screen::Devices => match client.request(&Command::ListNodes).await {
            Ok(Response::Nodes(nodes)) => format_devices(&nodes),
            Ok(_) => "Daemon returned an unexpected node response.".to_owned(),
            Err(error) => offline_message(error.kind()).to_owned(),
        },
        Screen::Diagnostics => match client.request(&Command::GetDiagnostics).await {
            Ok(Response::Diagnostics(snapshot)) => diagnostics::format_diagnostics(&snapshot),
            Ok(_) => "Daemon returned an unexpected diagnostic response.".to_owned(),
            Err(error) => offline_message(error.kind()).to_owned(),
        },
    }
}

/// Fetch each requested view on its own thread and send the rendered text back.
fn spawn_loader(results: Sender<app::Loaded>) -> io::Result<Sender<Screen>> {
    let (requests, incoming) = mpsc::channel::<Screen>();
    std::thread::Builder::new()
        .name("blueroute-tui-loader".to_owned())
        .spawn(move || {
            while let Ok(screen) = incoming.recv() {
                let body = futures_lite::future::block_on(fetch_view(screen));
                if results.send(app::Loaded { screen, body }).is_err() {
                    break;
                }
            }
        })?;
    Ok(requests)
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    loaded: &Receiver<app::Loaded>,
    signals: &Receiver<watch::Signal>,
    requests: &Sender<Screen>,
) -> io::Result<()> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};

    let mut state = app::App::default();
    for request in state.start() {
        let _ = requests.send(request);
    }
    while !state.should_quit() {
        terminal.draw(|frame| app::draw(frame, &state))?;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    for request in state.on_key(key) {
                        let _ = requests.send(request);
                    }
                }
                // Ratatui redraws at the new size on the next loop.
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        while let Ok(signal) = signals.try_recv() {
            for request in state.on_signal(signal) {
                let _ = requests.send(request);
            }
        }
        while let Ok(result) = loaded.try_recv() {
            state.on_loaded(result);
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("BlueRoute TUI requires an interactive text terminal.");
        std::process::exit(2);
    }
    let (results, loaded) = mpsc::channel();
    let requests = spawn_loader(results)?;
    let signals = watch::spawn()?;
    // `init` enters raw mode and the alternate screen and installs a panic hook that restores
    // the terminal; `restore` undoes it on every exit path below.
    let mut terminal = ratatui::init();
    let outcome = run(&mut terminal, &loaded, &signals, &requests);
    ratatui::restore();
    outcome
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
    fn overview_distinguishes_local_health_from_authenticated_peer_state() {
        let text = format_overview(&local_status(), None);
        assert!(text.contains("Healthy (local daemon only)"));
        assert!(text.contains("Connected devices: not reported"));
        assert!(text.contains("adapter availability unknown"));
        // No Internet sharing exists, so the overview does not mention it at all.
        assert!(!text.to_lowercase().contains("internet"));
        let peers = [NodeId::from_bytes([4; 16]), NodeId::from_bytes([5; 16])];
        let hosted = format_overview(&local_status(), Some(&peers));
        assert!(hosted.contains("Connected devices: 2 (authenticated control sessions"));
        assert!(format_overview(&local_status(), Some(&[])).contains("Connected devices: 0"));
    }

    #[test]
    fn network_and_node_views_use_only_explicit_daemon_records() {
        let id = NetworkId::from_bytes([9; 16]);
        let networks = vec![NetworkSummary {
            id,
            name: DisplayName::new("Local Lab").unwrap(),
            member_count: 2,
        }];
        let text = format_networks(&networks, Some(id));
        assert!(text.contains("Local Lab"));
        assert!(text.contains("not live peers"));
        // Actions are offered as shell commands, with the current network filled in.
        assert!(text.contains(&format!("Leave network {id}:  blueroute network leave")));
        assert!(!text.contains("network create"));
        let outside = format_networks(&[], None);
        assert!(outside.contains("No networks reported"));
        assert!(outside.contains("blueroute network create NAME"));
        assert!(outside.contains("Join a network:   not available yet"));
        assert!(!outside.contains("network leave"));
        let nodes = vec![NodeSummary {
            id: NodeId::from_bytes([3; 16]),
            name: DisplayName::new("This computer").unwrap(),
            reachability: Reachability::Local,
        }];
        assert!(format_devices(&nodes).contains("This computer"));
        assert!(format_devices(&nodes).contains("authenticated session"));
        assert!(format_devices(&nodes).contains("blueroute peer enroll NODE_ID FINGERPRINT"));
        assert!(format_devices(&nodes).contains("blueroute peer forget NODE_ID"));
        assert!(format_devices(&[]).contains("No nodes reported"));
    }

    #[test]
    fn unavailable_data_is_not_reported_as_observed_empty() {
        assert_eq!(
            diagnostics::observed_count::<u8>(None),
            "unavailable (not observed)"
        );
        assert_eq!(
            diagnostics::observed_count(Some(&[] as &[u8])),
            "0 reported"
        );
        assert!(offline_message(ClientErrorKind::Unavailable).contains("press r to retry"));
    }
}

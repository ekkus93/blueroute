//! Human and JSON output. JSON is the stable machine interface; text is for people.

use std::io::Write;

use blueroute_core::{HealthLevel, NodeCapabilities, PowerState, Reachability, Sourced};
use blueroute_protocol::{DaemonStatus, DiagnosticSnapshot, NetworkSummary, NodeSummary, Response};

use crate::args::Command;

pub fn render(command: &Command, json: bool, response: &Response, out: &mut dyn Write) {
    if json {
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string(response).expect("responses always serialize")
        );
        return;
    }
    let detailed = matches!(command, Command::Diagnostics(a) if a.detailed);
    match response {
        Response::Status(status) => status_text(status, out),
        Response::Capabilities(caps) => capability_text(caps, out),
        Response::Networks(networks) => networks_text(networks, out),
        Response::Nodes(nodes) => nodes_text(nodes, out),
        Response::Node(Some(node)) => nodes_text(std::slice::from_ref(node), out),
        Response::Node(None) => {
            let _ = writeln!(out, "No such computer in your network.");
        }
        Response::Diagnostics(snapshot) => diagnostics_text(snapshot, detailed, out),
        Response::Ack => {
            let _ = writeln!(out, "Done.");
        }
    }
}

pub fn health_word(health: HealthLevel) -> &'static str {
    match health {
        HealthLevel::Healthy => "working normally",
        HealthLevel::Degraded => "working, with limits",
        HealthLevel::Reconnecting => "reconnecting",
        HealthLevel::Error => "not working",
    }
}

fn reach_word(reach: Reachability) -> &'static str {
    match reach {
        Reachability::Local => "this computer",
        Reachability::DirectLink => "directly connected",
        Reachability::SamePanSegment => "nearby",
        Reachability::Routed => "through another computer",
        Reachability::Unreachable => "not reachable",
    }
}

fn status_text(status: &DaemonStatus, out: &mut dyn Write) {
    let _ = writeln!(out, "BlueRoute is {}.", health_word(status.health));
    match status.local_node {
        Some(id) => {
            let _ = writeln!(out, "This computer: {id}");
        }
        None => {
            let _ = writeln!(out, "This computer: (not identified yet)");
        }
    }
    match status.current_network {
        Some(id) => {
            let _ = writeln!(out, "Network: {id}");
        }
        None => {
            let _ = writeln!(out, "Network: not in a network");
        }
    }
}

fn yes_no(value: Option<&Sourced<bool>>) -> String {
    match value {
        None => "unknown".to_owned(),
        Some(v) => format!("{} ({:?})", if v.value { "yes" } else { "no" }, v.source),
    }
}

/// Label/value rows for every capability. Unknown stays distinct from "no".
pub fn capability_rows(caps: &NodeCapabilities) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        (
            "Bluetooth adapter usable",
            yes_no(caps.adapter_usable.as_ref()),
        ),
        ("Can join a network", yes_no(caps.panu.as_ref())),
        ("Can host a network", yes_no(caps.nap.as_ref())),
        (
            "Can pass traffic between networks",
            yes_no(caps.routing.as_ref()),
        ),
        ("Has Internet", yes_no(caps.has_internet.as_ref())),
        (
            "Willing to share Internet",
            yes_no(caps.willing_to_share_internet.as_ref()),
        ),
    ];
    rows.push((
        "Network manager",
        caps.network_backend
            .as_ref()
            .map_or("unknown".to_owned(), |b| {
                format!("{:?} ({:?})", b.value, b.source)
            }),
    ));
    rows.push((
        "Connection limit",
        caps.connection_policy_ceiling
            .as_ref()
            .map_or("unknown".to_owned(), |c| {
                format!("{} ({:?})", c.value, c.source)
            }),
    ));
    rows.push((
        "Power",
        caps.power_state
            .as_ref()
            .map_or("unknown".to_owned(), |p| match p.value {
                PowerState::AcPowered => "plugged in".to_owned(),
                PowerState::Battery { percent } => format!("battery {percent}%"),
            }),
    ));
    rows
}

fn capability_text(caps: &NodeCapabilities, out: &mut dyn Write) {
    for (label, value) in capability_rows(caps) {
        let _ = writeln!(out, "{label}: {value}");
    }
}

fn networks_text(networks: &[NetworkSummary], out: &mut dyn Write) {
    if networks.is_empty() {
        let _ = writeln!(out, "No networks found. Try `blueroute network discover`.");
    }
    for network in networks {
        let _ = writeln!(
            out,
            "{}  {}  {} member{}",
            network.name,
            network.id,
            network.member_count,
            if network.member_count == 1 { "" } else { "s" }
        );
    }
}

fn nodes_text(nodes: &[NodeSummary], out: &mut dyn Write) {
    if nodes.is_empty() {
        let _ = writeln!(out, "No other computers yet.");
    }
    for node in nodes {
        let _ = writeln!(
            out,
            "{}  {}  {}",
            node.name,
            node.id,
            reach_word(node.reachability)
        );
    }
}

fn diagnostics_text(snapshot: &DiagnosticSnapshot, detailed: bool, out: &mut dyn Write) {
    let _ = writeln!(out, "Health: {}", health_word(snapshot.health));
    let _ = writeln!(
        out,
        "Network: {}",
        snapshot
            .current_network
            .map_or("not in a network".to_owned(), |n| n.to_string())
    );
    let _ = writeln!(out, "Computers in view: {}", snapshot.visible_nodes);
    if detailed {
        let _ = writeln!(out, "Daemon API version: {}", snapshot.api_version);
        let _ = writeln!(out, "Capabilities:");
        for (label, value) in capability_rows(&snapshot.capabilities) {
            let _ = writeln!(out, "  {label}: {value}");
        }
    } else {
        let limits: Vec<&str> = capability_rows(&snapshot.capabilities)
            .into_iter()
            .filter(|(_, v)| v.starts_with("no"))
            .map(|(l, _)| l)
            .collect();
        if !limits.is_empty() {
            let _ = writeln!(out, "Not available here: {}", limits.join(", "));
        }
        let _ = writeln!(out, "Use --detailed for everything.");
    }
}

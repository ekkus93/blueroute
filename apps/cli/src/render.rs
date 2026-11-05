//! Human and JSON output. JSON is the stable machine interface; text is for people.

use std::io::Write;

use blueroute_core::HealthLevel;
use blueroute_protocol::{DaemonStatus, Response};

use crate::args::Command;

pub fn render(_command: &Command, json: bool, response: &Response, out: &mut dyn Write) {
    if json {
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string(response).expect("responses always serialize")
        );
        return;
    }
    match response {
        Response::Status(status) => status_text(status, out),
        Response::Ack => {
            let _ = writeln!(out, "Done.");
        }
        other => {
            let _ = writeln!(out, "{other:?}");
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

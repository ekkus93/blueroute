//! Byte-for-byte CLI behavior matrix: every command in the tree, with and without `--json`, plus
//! invalid arguments, help and version, run with no reachable daemon. Each case's stdout, stderr
//! and exit status are compared with a checked-in file under `tests/behavior/`.
//!
//! The system bus address names a socket that never exists, so every daemon command fails before
//! anything is sent. That address appears in connection diagnostics; it is replaced with `{bus}`
//! so expected files do not depend on the address.
//!
//! Expected files are rewritten only when `BLUEROUTE_BEHAVIOR_BLESS=1` is set. Review the diff of
//! `tests/behavior/` before committing a blessed change.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const NODE: &str = "00112233445566778899aabbccddeeff";
const FINGERPRINT: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const BLESS: &str = "BLUEROUTE_BEHAVIOR_BLESS";
/// A socket address under a directory that does not exist, so nothing can ever listen there.
const BUS: &str = "unix:path=/nonexistent/blueroute-behavior-no-such-bus";

/// Command lines in the order of the module-split capture; `{node}` and `{fp}` are substituted.
const COMMANDS: [&str; 31] = [
    "--help",
    "--version",
    "help network",
    "status",
    "capability show",
    "diagnostics show",
    "diagnostics show --summary",
    "network list",
    "network create Home",
    "network create",
    "network join {node}",
    "network leave",
    "network link",
    "network reconnect",
    "node list",
    "node show {node}",
    "node show not-a-node",
    "node set-name Laptop",
    "peer local-key",
    "peer verify-local-key {node} {fp}",
    "peer verify-local-key {node} {fp} --network {node}",
    "peer verify-local-key {node} short",
    "peer trust {node}",
    "peer enroll {node} {fp}",
    "peer forget {node}",
    "peer set-join-host {node} {node} {fp}",
    "peer clear-join-host",
    "discover start",
    "discover stop",
    "no-such-command",
    "network",
];

fn expected_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/behavior")
}

/// `NN-words[-json].txt`, from the case number and the command's leading words.
fn case_file(index: usize, command: &str, json: bool) -> String {
    let words: Vec<&str> = command
        .split_whitespace()
        .take_while(|word| !word.starts_with('{'))
        .map(|word| word.trim_start_matches('-'))
        .collect();
    let suffix = if json { "-json" } else { "" };
    format!("{:02}-{}{suffix}.txt", index + 1, words.join("-"))
}

fn run_case(command: &str, json: bool, bus: &str) -> String {
    let line = command.replace("{node}", NODE).replace("{fp}", FINGERPRINT);
    let mut arguments: Vec<&str> = line.split_whitespace().collect();
    if json {
        arguments.push("--json");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_blueroute"))
        .args(&arguments)
        .env("DBUS_SYSTEM_BUS_ADDRESS", bus)
        .output()
        .expect("CLI binary should run in the test environment");
    let status = output
        .status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| code.to_string());
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace(bus, "{bus}");
    format!(
        "argv: {}\nstatus: {status}\n--- stdout\n{}--- stderr\n{}",
        arguments.join(" "),
        text(&output.stdout),
        text(&output.stderr),
    )
}

/// Expected and actual lines that differ, by position, with one line of context either side.
fn line_diff(expected: &str, actual: &str) -> String {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let mut out = String::new();
    for index in 0..expected.len().max(actual.len()) {
        let (left, right) = (expected.get(index), actual.get(index));
        if left != right {
            if let Some(line) = left {
                out.push_str(&format!("  line {}: - {line}\n", index + 1));
            }
            if let Some(line) = right {
                out.push_str(&format!("  line {}: + {line}\n", index + 1));
            }
        }
    }
    if out.is_empty() {
        out.push_str("  (differs only in trailing newlines)\n");
    }
    out
}

#[test]
fn every_cli_command_matches_its_recorded_behavior_without_a_daemon() {
    // A fixed, short path of plain characters: the diagnostics print the address with some
    // characters percent-encoded, so a path taken from the checkout (with `+`, spaces or `~`)
    // would not match the `{bus}` replacement, and a long one could exceed the socket path limit.
    let bus = BUS.to_owned();
    assert!(
        !Path::new(bus.trim_start_matches("unix:path=")).exists(),
        "the no-daemon bus path must not exist: {bus}"
    );
    let bless = std::env::var(BLESS).is_ok_and(|value| value == "1");
    let dir = expected_dir();
    if bless {
        fs::create_dir_all(&dir).expect("create the behavior directory");
    }

    let mut cases = Vec::new();
    let mut failures = Vec::new();
    for (index, command) in COMMANDS.iter().enumerate() {
        for json in [false, true] {
            let name = case_file(index, command, json);
            let actual = run_case(command, json, &bus);
            let path = dir.join(&name);
            if bless {
                fs::write(&path, &actual).expect("write an expected behavior file");
            } else {
                match fs::read_to_string(&path) {
                    Ok(expected) if expected == actual => {}
                    Ok(expected) => {
                        failures.push(format!("{name}:\n{}", line_diff(&expected, &actual)))
                    }
                    Err(error) => failures.push(format!("{name}: cannot read: {error}")),
                }
            }
            cases.push(name);
        }
    }

    let mut unexpected: Vec<String> = fs::read_dir(&dir)
        .expect("read the behavior directory")
        .map(|entry| {
            entry
                .expect("behavior entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !cases.contains(name))
        .collect();
    unexpected.sort();
    for name in unexpected {
        failures.push(format!("{name}: no case produces this file; remove it"));
    }

    assert_eq!(cases.len(), 62, "case count");
    assert!(
        failures.is_empty(),
        "{} of {} CLI behavior cases differ (set {BLESS}=1 to rewrite after review):\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

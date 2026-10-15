//! Executable CLI exit-contract tests; no production daemon or Bluetooth
//! hardware is required. All errors here are intentionally local/synthetic.

use std::process::{Command, Output};

fn invoke(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blueroute"))
        .args(arguments)
        .output()
        .expect("CLI binary should run in test environment")
}

#[test]
fn help_and_version_do_not_require_daemon_and_exit_zero() {
    for args in [&["--help"][..], &["--version"][..]] {
        let result = invoke(args);
        assert_eq!(result.status.code(), Some(0), "args={args:?}");
        assert!(result.stderr.is_empty());
    }
}

#[test]
fn invalid_and_duplicate_options_exit_usage_without_contacting_daemon() {
    for args in [
        &["--unrecognized"][..],
        &["--json", "--json", "status"][..],
        &["diagnostics"][..],
        &["network", "join"][..],
        &["diagnostics", "show", "unexpected"][..],
    ] {
        let result = invoke(args);
        assert_eq!(result.status.code(), Some(2), "args={args:?}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("Usage: blueroute"),
            "args={args:?}: {stderr}"
        );
    }
}

#[test]
fn daemon_unavailable_exits_three_without_falling_back_to_privileged_tools() {
    // /dev/null is not a D-Bus socket. Even on a developer machine with a
    // functioning real system bus this forces local connection failure.
    let result = Command::new(env!("CARGO_BIN_EXE_blueroute"))
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/dev/null")
        .args(["diagnostics", "show", "--json"])
        .output()
        .expect("CLI binary should run in test environment");

    assert_eq!(result.status.code(), Some(3));
    assert!(result.stdout.is_empty());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("daemon unavailable/incompatible"),
        "unexpected connection diagnostic: {error}"
    );
}

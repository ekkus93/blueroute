//! Script-level checks of the exit-code contract (P10-006) against the real binary.

use std::process::Command;

fn blueroute() -> Command {
    Command::new(env!("CARGO_BIN_EXE_blueroute"))
}

#[test]
fn version_and_help_exit_zero() {
    for flag in ["--version", "--help"] {
        let output = blueroute().arg(flag).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{flag}");
        assert!(!output.stdout.is_empty());
    }
}

#[test]
fn usage_errors_exit_two_and_print_to_stderr() {
    for args in [vec![], vec!["frobnicate"], vec!["network", "join"]] {
        let output = blueroute().args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn unreachable_system_bus_exits_three_with_a_hint() {
    let output = blueroute()
        .arg("status")
        .env(
            "DBUS_SYSTEM_BUS_ADDRESS",
            "unix:path=/nonexistent/blueroute-test-bus",
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("blueroute: "), "{stderr}");
    assert!(output.stdout.is_empty());
}

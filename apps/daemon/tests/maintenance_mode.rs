//! CR-F5: the daemon binary's argument contract and its startup refusals. These run the real
//! executable and never reach a bus: every case is refused before the daemon would connect.
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use blueroute_daemon::{REMOVAL_PENDING_FILE, acquire_instance_lock};

fn daemon(state: &PathBuf, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blueroute-daemon"))
        .args(arguments)
        .env("BLUEROUTE_STATE_DIR", state)
        .output()
        .expect("the daemon binary runs")
}

fn state(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "blueroute-maintenance-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn only_the_fixed_maintenance_flag_is_accepted() {
    let state = state("arguments");
    for arguments in [
        &["--help"][..],
        &["--remove-owned-networking", "extra"],
        &["/bin/sh"],
    ] {
        let output = daemon(&state, arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(stderr(&output).contains("usage"), "{arguments:?}");
    }
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn removal_cleanup_is_refused_to_anyone_but_root() {
    if std::fs::metadata("/proc/self").unwrap().uid() == 0 {
        return; // Only meaningful for an unprivileged test run.
    }
    let state = state("non-root");
    let output = daemon(&state, &["--remove-owned-networking"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("must run as root"));
    // Refused before touching the state directory.
    assert!(!state.join(REMOVAL_PENDING_FILE).exists());
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_pending_removal_keeps_the_daemon_from_starting() {
    let state = state("pending");
    std::fs::write(state.join(REMOVAL_PENDING_FILE), b"removing\n").unwrap();
    let output = daemon(&state, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("being removed"),
        "{}",
        stderr(&output)
    );
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_held_instance_lock_keeps_a_second_daemon_from_starting() {
    let state = state("locked");
    let _running = acquire_instance_lock(&state).unwrap();
    let output = daemon(&state, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("stop blueroute.service"),
        "{}",
        stderr(&output)
    );
    std::fs::remove_dir_all(state).unwrap();
}

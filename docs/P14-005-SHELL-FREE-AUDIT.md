# P14-005 — Shell-free privileged-path audit

**Reviewed:** 2026-10-15

## Scope and findings

The production Rust paths under `apps/` and `crates/` were searched for `Command::new`, `std::process::Command`, shell invocation patterns (`sh -c`, `bash -c`), and legacy `nmcli`/`bluetoothctl` execution. The only production process launch is in `crates/blueroute-linux/src/capability_report.rs`: it invokes a fixed list of known `bluetoothd` binary paths (then `bluetoothd` via `PATH`) with the fixed `--version` argument to report a version when BlueZ does not expose one over D-Bus.

No shell is started. No peer, user, device name, D-Bus field, or CLI argument is interpolated into that command. BlueZ and NetworkManager operations use their typed D-Bus APIs; NetworkManager source explicitly documents that production does not invoke or parse `nmcli`.

The P6-006 Python loopback acceptance harness uses `subprocess.Popen` with an argument vector and the default `shell=False` to start local test binaries. It is test tooling, not a privileged production path. The other P6-006 scripts exercise raw sockets or Linux networking acceptance helpers and do not construct shell commands.

## Maintenance rule

Any future subprocess call in production must use a fixed executable and separately supplied arguments, never a shell, and must not derive executable names or arguments from peer-controlled or user-controlled strings. Prefer the existing BlueZ/NetworkManager D-Bus adapters for networking operations. Re-run the source search when adding privileged operations or process-launch dependencies.

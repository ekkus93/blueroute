# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Rust workspace (edition 2024, toolchain pinned to 1.95.0). Only the desktop frontend in `apps/desktop` uses Node (pinned: Node 26.13.0, npm 11.20.0; always `npm ci`). Building the workspace needs the GTK/WebKit dev libraries listed in `docs/DEVELOPMENT.md`. Python 3 is used only for `scripts/test_p6_006_*.py`.

## Required checks (CI runs all of these with `--locked`)

```
cargo fmt --all -- --check        # fix with: cargo fmt --all
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Integration tests are `#[ignore]` and need a private bus. Run each as:

```
dbus-run-session -- cargo test -p <pkg> --test <file> --locked -- --ignored --exact <name>
```

- `-p blueroute-daemon --test dbus_service dbus_service_round_trip_and_event`
- `-p blueroute-daemon --test authorization authorization_is_read_only_by_default_and_polkit_gates_mutations`
- `-p blueroute-client --test dbus_client client_queries_events_reconnects_and_fails_closed_on_incompatibility`
- `-p blueroute --test cli_exit_contract cli_exit_statuses_propagate_typed_broker_failures`

Desktop frontend checks (run in `apps/desktop`, also in CI): `npm run format:check`, `npm run typecheck`, `npm test`, `npm run build`.

Max line width is 100. The workspace forbids `unsafe_code`, and `dbg!` is denied. Warnings fail CI.

## Architecture rules

- `apps/*` (daemon, cli, tui, desktop) are front ends. They must contain no Bluetooth or routing logic and go through the daemon API (`org.blueroute.Service1` on the system D-Bus) via `blueroute-client`.
- Hardware-specific code stays behind the capability and Linux adapter boundaries in `crates/blueroute-linux`. Domain logic goes in `blueroute-core`, shared types in `blueroute-protocol`.
- Changing an invariant requires updating `docs/SPEC.md` first or in the same change.
- Production code never shells out (no `nmcli`/`bluetoothctl`); see `docs/P14-005-SHELL-FREE-AUDIT.md`.
- Never touch foreign routes, devices, IPs or state. Cleanup is limited to resources carrying BlueRoute ownership metadata (NetworkManager `user.data`, scoped by NetworkId).
- Internet sharing (P18) stays off for v1.

## Fail-closed security model

- Production JoinNetwork stays blocked until authenticated P7 transport exists.
- Advertisements, MAC/IP claims, display names and trust-only approvals are NOT authenticated membership.
- Authorization binds to NetworkId, NodeId, an independently confirmed SPKI SHA-256, a TLS 1.3 peer-key proof, an exporter-bound Hello, and current durable approval.
- The desktop bridge is read-only except `create_network` and `leave_network` (the only Tauri mutations; the daemon still enforces PolicyKit): unknown stays unknown, the platform check treats "unknown" as unsupported, Join is disabled, and internet sharing is hidden.

## Testing

- Deterministic tests must run without Bluetooth hardware. Physical behavior needs separate hardware evidence (`docs/HARDWARE_ACCEPTANCE_BACKLOG.md`). Never generalize one adapter's result to others.
- Use the platform terms Supported / Tested / Experimental / Unsupported (defined in `CONTRIBUTING.md`) consistently.

## Tasks, commits and docs

- `docs/TODO.md` is the authoritative backlog (task IDs like P12-004). `docs/SPEC.md` holds the architecture.
- Do not tick a TODO checkbox until its acceptance criteria are met. Keep "SOFTWARE COMPLETE / HARDWARE DEFERRED" distinct from full COMPLETE. Older status paragraphs may be stale, so prefer the code and later dated evidence.
- Commits use Conventional Commits with a task scope (`feat(desktop): ...`, `docs(P12): ...`) and reference the task ID.
- Workflow: commit directly to master, then verify CI on the exact pushed head. Run `cargo fmt --all` before committing.

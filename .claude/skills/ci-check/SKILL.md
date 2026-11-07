---
name: ci-check
description: Run the BlueRoute CI checks locally (fmt, check, clippy, test, and the ignored D-Bus integration tests) and report failures.
disable-model-invocation: true
---

Run these in order from the repo root and stop at the first failure, reporting the exact output. Do not "fix" failures by weakening tests or lints.

1. `cargo fmt --all -- --check` (if it fails, run `cargo fmt --all` and re-check)
2. `cargo check --workspace --locked`
3. `cargo clippy --workspace --all-targets --locked -- -D warnings`
4. `cargo test --workspace --locked`
5. The ignored D-Bus integration tests, each under a private bus:
   - `dbus-run-session -- cargo test -p blueroute-daemon --test dbus_service --locked -- --ignored --exact dbus_service_round_trip_and_event`
   - `dbus-run-session -- cargo test -p blueroute-daemon --test authorization --locked -- --ignored --exact authorization_is_read_only_by_default_and_polkit_gates_mutations`
   - `dbus-run-session -- cargo test -p blueroute-client --test dbus_client --locked -- --ignored --exact client_queries_events_reconnects_and_fails_closed_on_incompatibility`
   - `dbus-run-session -- cargo test -p blueroute --test cli_exit_contract --locked -- --ignored --exact cli_exit_statuses_propagate_typed_broker_failures`
6. If scripts/ changed: `python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v`

Finish with a pass/fail summary per step. Packaging checks (systemd-analyze, Debian build) need sudo and run in CI only; mention them as not run locally.

---
name: lint-n-test
description: Lint the workspace (rustfmt, clippy) and run all tests, including the ignored D-Bus integration tests.
model: haiku
disable-model-invocation: true
---

Lint the code, then run every test. Run from the repo root and report exact output for any failure. Do not weaken lints or tests to make them pass.

**Lint**
1. `cargo fmt --all -- --check` (if it fails, run `cargo fmt --all`, then re-check)
2. `cargo clippy --workspace --all-targets --locked -- -D warnings`

**Test**
3. `cargo test --workspace --locked`
4. The ignored D-Bus integration tests, each under a private bus:
   - `dbus-run-session -- cargo test -p blueroute-daemon --test dbus_service --locked -- --ignored --exact dbus_service_round_trip_and_event`
   - `dbus-run-session -- cargo test -p blueroute-daemon --test authorization --locked -- --ignored --exact authorization_is_read_only_by_default_and_polkit_gates_mutations`
   - `dbus-run-session -- cargo test -p blueroute-client --test dbus_client --locked -- --ignored --exact client_queries_events_reconnects_and_fails_closed_on_incompatibility`
   - `dbus-run-session -- cargo test -p blueroute --test cli_exit_contract --locked -- --ignored --exact cli_exit_statuses_propagate_typed_broker_failures`
5. `python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v`

Run all steps even if an earlier one fails, so the report is complete. End with a pass/fail line per step.

---
name: lint-n-test
description: Lint the files and run all tests. Use when the user asks to lint and test, or runs /lint-n-test.
model: haiku
---

From the repo root, run these in order and report the result of each (pass/fail, with the relevant output on failure):

1. Lint: `cargo fmt --all -- --check`, then `cargo clippy --workspace --all-targets --locked -- -D warnings`
2. Tests: `cargo test --workspace --locked`
3. Ignored D-Bus tests (need `dbus-run-session`; say so clearly if unavailable):
   - `dbus-run-session -- cargo test -p blueroute-daemon --test dbus_service --locked dbus_service_round_trip_and_event -- --ignored --exact`
   - `dbus-run-session -- cargo test -p blueroute-daemon --test authorization --locked authorization_is_read_only_by_default_and_polkit_gates_mutations -- --ignored --exact`
   - `dbus-run-session -- cargo test -p blueroute-client --test dbus_client --locked client_queries_events_reconnects_and_fails_closed_on_incompatibility -- --ignored --exact`

Run every step even if an earlier one fails, so the report covers everything. Don't fix anything unless asked; just report. Never claim success for a skipped step.

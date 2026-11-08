---
name: lint-n-test
description: Lint and test the whole repo - Rust workspace (rustfmt, clippy, tests, ignored D-Bus integration tests), Python scripts, and the desktop frontend (prettier, tsc, vitest, build).
model: haiku
disable-model-invocation: true
---

Lint the code, then run every test. Run from the repo root and report exact output for any failure. Do not weaken lints or tests to make them pass.

**Rust lint**
1. `cargo fmt --all -- --check` (if it fails, run `cargo fmt --all`, then re-check)
2. `cargo clippy --workspace --all-targets --locked -- -D warnings`

**Rust and Python tests**
3. `cargo test --workspace --locked --no-fail-fast`
4. The ignored D-Bus integration tests, each under a private bus:
   - `dbus-run-session -- cargo test -p blueroute-daemon --test dbus_service --locked -- --ignored --exact dbus_service_round_trip_and_event`
   - `dbus-run-session -- cargo test -p blueroute-daemon --test authorization --locked -- --ignored --exact authorization_is_read_only_by_default_and_polkit_gates_mutations`
   - `dbus-run-session -- cargo test -p blueroute-client --test dbus_client --locked -- --ignored --exact client_queries_events_reconnects_and_fails_closed_on_incompatibility`
   - `dbus-run-session -- cargo test -p blueroute --test cli_exit_contract --locked -- --ignored --exact cli_exit_statuses_propagate_typed_broker_failures`
5. `python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v`

**Desktop frontend** (run from `apps/desktop`; Node 26.13.0 / npm 11.20.0 are pinned, see `docs/DEVELOPMENT.md`)
6. `npm ci` (locked install; if the Node or npm version does not match, report it instead of working around it)
7. `npm run format:check` (if it fails, run `npm run format`, then re-check)
8. `npm run typecheck`
9. `npm test`
10. `npm run build`

Run all steps even if an earlier one fails, so the report is complete. If a flaky-looking daemon test fails in the full workspace run, re-run that single test to see whether it is load-related, but still report the original failure. End with a pass/fail line per step.

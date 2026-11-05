---
name: check
description: Run the same quality gate as CI (fmt, check, clippy, test, and the ignored D-Bus tests) and report results accurately. Use before handing off or committing Rust changes.
---

Run these from the repo root, in order, and stop to report on the first failure (with the relevant output):

1. `cargo fmt --all -- --check`
2. `cargo check --workspace --locked`
3. `cargo clippy --workspace --all-targets --locked -- -D warnings`
4. `cargo test --workspace --locked`
5. The ignored D-Bus tests (need `dbus-run-session`; skip with a clear note if it is unavailable):
   - `dbus-run-session -- cargo test -p blueroute-daemon --test dbus_service --locked dbus_service_round_trip_and_event -- --ignored --exact`
   - `dbus-run-session -- cargo test -p blueroute-daemon --test authorization --locked authorization_is_read_only_by_default_and_polkit_gates_mutations -- --ignored --exact`
   - `dbus-run-session -- cargo test -p blueroute-client --test dbus_client --locked client_queries_events_reconnects_and_fails_closed_on_incompatibility -- --ignored --exact`

If `.github/workflows/ci.yml` differs from the above, follow CI. Report pass/fail per step; don't claim success for skipped steps.

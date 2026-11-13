# Code Review Remediation Evidence

Durable evidence for [CODE_REVIEW_REMEDIATION_TODO.md](CODE_REVIEW_REMEDIATION_TODO.md). The
requirements are in [CODE_REVIEW_REMEDIATION_SPEC.md](CODE_REVIEW_REMEDIATION_SPEC.md). Entries
are appended as tasks are qualified; earlier entries are not rewritten.

## CR-BASE — baseline and reproductions (2026-11-13)

### CR-BASE-01: baseline recheck

- HEAD is `4ca5042ee28743dd30782379fbcf8fa1ed15df77`, identical to the reviewed baseline.
- Working tree before remediation: only untracked `.serena/` (never committed), `AGENTS.md`, and
  the two remediation documents. No tracked file differed from the baseline.
- Applicable instructions: `CLAUDE.md` and `AGENTS.md` agree (pinned toolchains, locked checks,
  Conventional Commits with task scopes, oxlint findings fixed without disabling rules, ignored
  D-Bus tests run with `dbus-run-session`, TODO items ticked only on accepted evidence).
- The code at HEAD still matches every finding's description (confirmed by the reproductions
  below and the static reading for F5 and F7).

### CR-BASE-02/03: findings and repository reproductions

Each reproducible finding now has a repository test that states the **corrected** behavior and
fails on the baseline. The tests are committed disabled so the required checks stay green:
Rust tests carry `#[ignore = "CR-Fx regression: …"]`, the UI test uses vitest `test.fails`.
Each fix task re-enables its tests; the commit that does so is that finding's evidence. The
baseline failures below were observed at `4ca5042` plus only these test files.

| Finding | Reproduction | Command | Observed baseline failure |
| --- | --- | --- | --- |
| F1 | `apps/daemon/tests/state_events.rs` `cr_f1_a_worker_health_change_reaches_a_connected_subscriber`: a real `DaemonService` on a private bus publishes a runtime health observation through `update_runtime_observation`; a separate connection waits for the `Event` signal. | `dbus-run-session -- cargo test -p blueroute-daemon --test state_events --locked -- --ignored` | `no event reached the subscriber within 1s` (published health was already `Degraded`). |
| F2 | `crates/blueroute-client/tests/owner_watch.rs` `cr_f2_idle_daemon_loss_ends_the_event_subscription_without_any_event`: subscribe, then the daemon releases its name and disconnects without emitting an Event. | `dbus-run-session -- cargo test -p blueroute-client --test owner_watch --locked -- --ignored` | `the subscription did not notice the owner loss within 2s`. |
| F3 | `crates/blueroute-core/src/topology.rs` `cr_f3_*` (4 tests): identical segment reinsertion, segment NAP change under links, LinkId rebinding, and a whole-graph consistency check. | `cargo test -p blueroute-core --locked --lib cr_f3 -- --ignored` | Reinsertion drops the attached client; NAP change and rebinding are accepted; a link's NAP no longer matches its segment. |
| F4 | `apps/daemon/src/single_star.rs` `cr_f4_recovered_hosting_capability_unblocks_create_without_restart`: one `DaemonService` + `SingleStarNetworkOperations` (fake runtime) starts incapable, then publishes a usable observation. | `cargo test -p blueroute-daemon --locked --lib cr_f4 -- --ignored` | Status reports `can_host_pan() == Some(true)`, yet create fails `NotSupported("this Linux node does not expose BlueZ NAP capability")`; the runtime is never reached. |
| F6 | `apps/desktop/ui/src/useDaemon.test.tsx` "CR-F6: every subscription is released exactly once …": `useDaemon` under `StrictMode` with deferred `subscribe` results, resolved after the replay, then unmount. | `npx vitest run ui/src/useDaemon.test.tsx` (in `apps/desktop`; as `test.fails` it reports "expected fail") | Run as a plain test: `expected [ +0, 1 ] to deeply equal [ 1, 1 ]`: the first subscription is never released. |
| F8 | `crates/blueroute-client/tests/owner_race.rs` `cr_f8_incompatible_replacement_during_version_check_receives_no_command`: compatible owner A hands the name to API-99.0 owner B inside its request-time `Version` reply; B counts `Request` calls. | `dbus-run-session -- cargo test -p blueroute-client --test owner_race --locked -- --ignored` | B received the `CreateNetwork` request (`left: 1, right: 0`), and the client reported `Ok(Ack)` from B. |

The handover in F8 happens inside A's handler, so it is deterministic, not timing-dependent. The
F1 and F2 bounds are the spec's controlled-environment bounds (EVENT-6, WATCH-4).

### CR-BASE-04: F5 static-evidence basis

F5 is a missing cleanup path, so its basis is static, as the specification states:

- `packaging/debian/prerm` only runs `deb-systemd-invoke stop blueroute.service … || true` on
  remove/deconfigure; failures are swallowed.
- `packaging/debian/postrm` only disables the unit on remove and deletes `/var/lib/blueroute`
  on purge. Nothing removes the bridge, address or NetworkManager profile the daemon created.
- `packaging/systemd/blueroute.service` has no `ExecStop`, and `RuntimeDirectory=blueroute` means
  systemd deletes `/run/blueroute` on stop, which is where removal-time evidence such as a
  forwarding lease must survive (CLEAN-4).
- The isolated upgrade test (`packaging/debian/test-package-upgrade.sh`) mocks the systemd
  helpers and checks file removal and state retention only; it cannot show network state.

Fake-backend fixtures for owned/foreign profiles and addresses already exist
(`apps/daemon/src/client_leave.rs` tests; `crates/blueroute-linux` forwarding lease tests). The
remaining fixtures (interrupted cleanup, stopped daemon, the cleanup entry point itself) depend on
the entry-point design that CR-F5-01 must record first, so CR-BASE-04 stays open until then.

### CR-BASE-05: F7 discrepancies at the baseline

| Location | Claim | Actual |
| --- | --- | --- |
| `docs/SPEC.md` (current implementation, `blueroute-protocol`) | "API 1.5" | `ApiVersion::CURRENT` is 1.6 (`crates/blueroute-protocol/src/lib.rs`); `docs/P5-PROTOCOL.md` correctly says 1.6. |
| `docs/SPEC.md` (incomplete behavior) | "The package ships the daemon and CLI only" | `blueroute` ships daemon, CLI and TUI; `blueroute-desktop` is a separate package (`packaging/debian/build-package.sh`). |
| `docs/TODO.md` status table, P15-003 | "the desktop application is still a placeholder" | A functional desktop app is packaged as `blueroute-desktop`. |
| `docs/TODO.md` status table header and P6-004/P7-003/P7-004 rows | Dated 2026-10-16; join preflight "pending … PANU/control wiring"; enrollment UX and live transport open | Join path, inbound listener, client supervisor, reconnection and typed enrollment on both sides exist in software behind the closed join gate (later dated notes in the same file). |
| `docs/ADR-INDEX.md` | Lists only P7-001; P7-001 says "user-verified enrollment UX and live daemon transport integration remain open" | The accepted P8-009 ADR (`docs/P8-009-DYNAMIC-ROUTING-ADR.md`) is missing; typed enrollment and the daemon transport now exist in software (hardware unqualified). |
| `docs/DEVELOPMENT.md` | "The placeholder crates intentionally contain almost no behavior" | All crates are implemented. |

Membership schema v3 (`CURRENT_FORMAT_VERSION = 3`) is described correctly where stated. Dated
historical hardware evidence is correct at its recorded scope and is not a discrepancy.

### CR-BASE-06: isolation

Every new reproduction runs without the host package database, system services, Bluetooth
controllers or user networking: the D-Bus tests use a private session bus from
`dbus-run-session` (never the system bus); the daemon tests use temporary directories and the
fake star runtime; the graph tests are pure; the UI test runs in jsdom with fake APIs.

## CR-F8 — owner-bound operations (2026-11-13)

**Fix:** `b3ba0bdc9e35393a8bbcf16affc845b37b03b0f0`. `crates/blueroute-client/src/lib.rs` resolves
`org.blueroute.Service1` to its unique owner (`GetNameOwner`) once per operation, checks
`Version` at that unique name, and sends `Request` to the same unique name. `events()` subscribes
with that unique name as the signal sender, and `next_event()` rechecks that same owner.
`server_version()` is now documented as the last validated owner's version (informational; every
operation validates its own owner). No automatic retry was added; `reconnect()` still retries
only the read-only owner/version check.

**Regressions** (`crates/blueroute-client/tests/owner_race.rs`, private bus, serialized because
they share the well-known name; added to CI):

| Test | Requirement | Result at fix | With baseline client restored |
| --- | --- | --- | --- |
| `cr_f8_incompatible_replacement_during_version_check_receives_no_command` | OWN-1/2: API 99.0 owner B gets the name inside A's request-time `Version`; B receives zero Requests; A (relinquished, still connected) finishes the command | pass | **fail** (`left: 1, right: 0`) |
| `cr_f8_a_subscription_never_delivers_a_replacement_owners_events` | OWN-4: after A→B handover, B's event never arrives on A's subscription; A's does | pass | **fail** (B's event delivered) |
| `cr_f8_after_a_compatible_replacement_the_next_command_goes_to_the_new_owner` | OWN-2: a fresh operation validates and uses the new compatible owner | pass | pass |
| `cr_f8_an_owner_vanishing_before_dispatch_fails_closed_and_reaches_no_one` | OWN-2: owner closes inside `Version`; typed `Unavailable`, no Request delivered | pass | pass |
| `cr_f8_a_lost_reply_is_reported_and_never_replayed` | OWN-3: A never replies and disconnects; error reported, A got exactly one Request, compatible successor B got none | pass | pass |

The last three guard behavior that must not regress; the first two are the defect. Three
consecutive runs passed. The fix was confirmed by restoring the baseline `lib.rs`, running the
file (2 failed, 3 passed), and restoring the fix.

**Checks at the fix (local):** `cargo fmt --all -- --check`, `cargo clippy --workspace
--all-targets --locked -- -D warnings`, `cargo test --workspace --locked` all pass; private-bus
suites `dbus_service`, `authorization`, `dbus_client`, `cli_exit_contract`, `bridge_end_to_end`
and `owner_race` all pass.

**CR-F8-06 call-site review:** the CLI and the desktop bridge build a fresh client per command,
so every command is owner-bound. The desktop (`apps/desktop/src/watch.rs`) and TUI
(`apps/tui/src/watch.rs`) watchers loop on `events()`: with owner binding a replacement's events
no longer arrive on an old subscription, but the stream still does not end when its owner goes
away. That is F2 (CR-F2), not an F8 assumption. The daemon does not use the client.

## CR-F2 — owner-loss detection and watcher recovery (2026-11-13)

**Fix:** `e283b4ed8bba988e50b3cb7fdb966e2ac780cbb5`.

- `EventSubscription` (`crates/blueroute-client/src/lib.rs`) also holds a `NameOwnerChanged`
  stream filtered to `org.blueroute.Service1` (arg0 match rule, so unrelated names never reach
  it). It is installed *before* the owner is resolved; a change queued during setup that leaves
  the name with another owner ends the subscription. `next_event()` races that stream against
  the Event stream and returns the new `ClientError::OwnerLost` (kind `Unavailable`) when the
  bound owner gives the name up or vanishes; a closed connection ends both streams. A
  notice that merely records the bound owner taking the name is ignored. If an earlier,
  superseded transition is still queued at setup the result is a spurious `OwnerLost`, which the
  watcher answers with an ordinary reconnect: fail-safe, never a missed loss.
- `blueroute_client::watch` (`crates/blueroute-client/src/watch.rs`) is the one loop both front
  ends run (`apps/desktop/src/watch.rs`, `apps/tui/src/watch.rs` are thin adapters). `connect`
  and `sleep` are injected; production passes `BlueRouteClient::connect` and `Timer::after`.
  It reports `Disconnected` once per loss (not per failed attempt), keeps the 1 s → 30 s
  schedule, resets it after a successful connection, and returns, dropping its subscription,
  when the consumer returns `Break` (the TUI does once its receiver is gone).
- WATCH-3: the desktop banner now says "What is shown below may be out of date until the service
  is back"; the TUI status line says "Lost contact; view may be out of date…" (fits the 40-column
  minimum). Both front ends also reread on `Disconnected`, which replaces retained data with the
  unavailable explanation as soon as the reads fail.
- No healthy-state polling: a live subscription only awaits signals (WATCH-4).

**Regressions** (private bus; serialized per file; all added to CI):

| Test | Covers | Result |
| --- | --- | --- |
| `owner_watch::cr_f2_idle_daemon_loss_ends_the_event_subscription_without_any_event` | WATCH-1 idle loss, no Event; `OwnerLost` within 2 s | pass (baseline: **fail**, see CR-BASE) |
| `owner_watch::cr_f2_an_unrelated_name_changing_hands_does_not_disturb_the_subscription` | WATCH-2 unrelated names | pass |
| `owner_watch::cr_f2_losing_the_bus_connection_ends_the_subscription` | bus loss | pass |
| `owner_race::cr_f2_an_owner_change_during_subscription_setup_is_not_missed` | WATCH-1 setup race (handover inside the `events()` version check) | pass |
| `owner_race::cr_f8_a_subscription_never_delivers_a_replacement_owners_events` | OWN-4 / WATCH-1 replacement ends the old subscription with `OwnerLost`, never delivering the replacement's event | pass |
| `watcher::cr_f2_idle_loss_is_reported_and_a_compatible_return_reconnects` | WATCH-2/4 idle stop → `Disconnected` (measured ≈0.5 ms from stop, bound 2 s) → compatible return → `Connected` → one event gives exactly one `Changed` (no duplicate listener); first backoff 1 s | pass |
| `watcher::cr_f2_direct_replacement_moves_the_watcher_to_the_new_owner` | direct replacement; the retired owner's later event is not heard | pass |
| `watcher::cr_f2_an_incompatible_return_stays_unavailable` | API 99.0 owner never connects or delivers; compatible return then connects | pass |
| `watcher::cr_f2_backoff_keeps_the_production_schedule_and_reports_loss_once` | injected clock: requested delays 1, 2, 4, 8, 16, 30, 30, 30 s; one `Disconnected` | pass |
| `watcher::cr_f2_the_watcher_stops_when_its_consumer_does` | cancellation/consumer shutdown | pass |
| TUI `app.rs` link test, desktop `App.test.tsx` reconnect test | WATCH-3 stale labelling, cleared on reconnect | pass |

`watcher` passed three consecutive runs (≈0.5 s each).

**Checks at the fix (local):** fmt, clippy `-D warnings`, `cargo test --workspace --locked`;
private-bus `dbus_service`, `authorization`, `dbus_client`, `cli_exit_contract`,
`bridge_end_to_end`, `owner_race`, `owner_watch`, `watcher`; desktop `format:check`, `lint`,
`typecheck`, `test` (34 passed, the CR-F6 expected failure), `build`.

**Scope note:** the desktop's own subscription lifetime under StrictMode is CR-F6, which builds
on this watcher contract.

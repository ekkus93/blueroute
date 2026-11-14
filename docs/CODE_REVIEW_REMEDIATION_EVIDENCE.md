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

## CR-F1 — publish actual daemon changes (2026-11-13)

**Decision (user, 2026-11-13):** a new payload-free D-Bus signal `StateChanged(revision u64)`,
API 1.7, rather than new `Event` variants (which 1.6 clients would fail to decode) or stretched
existing events. Documented in `docs/P5-PROTOCOL.md` (API 1.7) and `docs/SPEC.md` §11 before
acceptance.

**Fix:** `4de29b19432d9b5447087e93e4d7cc31ecfe5232`.

CR-F1-01 inventory and policy (`apps/daemon/src/state_publisher.rs`):

| Observable snapshot | Producer | How it is published |
| --- | --- | --- |
| Status: current network | request dispatch (create, join where enabled, leave) | `committed()` after the request, success or failure |
| Status/diagnostics: health, capabilities, system | runtime worker (`update_runtime_observation`, `update_runtime_health`) | `observed()` after the locks are released, only if changed |
| Health + `GetClientLink` phase | client reconnector observer | `observed()` on every phase, even with unchanged health |
| Nodes / diagnostics: authenticated control peers | control worker | 250 ms in-memory recheck (no mark) |
| Nodes: device name | `SetDeviceName` | `committed()` |
| Diagnostics: recent failures | request dispatch | `committed()` for mutations; `observed()` for failed reads |
| Networks (membership store, live discovery) | requests; background store/discovery changes | `committed()`; 1 s network recheck |
| Trust, pins, join host records (not shown in snapshots but committed) | requests | `committed()` |

Policy: one emitter thread; marks set an atomic flag and unpark it (bounded, coalescing, cannot
drop the last invalidation); snapshots are cloned under their locks, compared as encoded
responses, and the signal is emitted with no lock held. Ordering: revisions increase per daemon
process; duplicates are possible after a forced (committed) publication with no visible change;
receivers only reread. Failure: payload-free `state_changed_signal` log on the first failure and on
recovery; the invalidation stays pending and is retried on every recheck; the mutation's result is
never affected. `DiagnosticSnapshot.interfaces` (read live from NetworkManager on request) is not
rechecked.

CR-F1-07: the shared watcher reports `Connected` only after a fresh owner-bound subscription
exists, and both front ends reread on `Connected` (TUI `app.rs` link test; desktop "rereads once
the live subscription is up" test), so a change between the first read and the subscription is
not missed, and every new compatible owner is reread rather than replayed.

**Regressions:**

| Test | Covers | Result |
| --- | --- | --- |
| `state_events::cr_f1_a_worker_health_change_reaches_a_connected_subscriber` | worker → signal → reread; ≈0.3 ms | pass (baseline **fail**: nothing in 1 s) |
| `state_events::cr_f1_another_clients_committed_create_and_leave_reach_every_observer` | committed create and leave by another client reach two independent observers; reread shows committed state; ≈2 ms | pass |
| `state_events::cr_f1_a_rename_is_announced` | `SetDeviceName` | pass |
| `state_events::cr_f1_a_refused_mutation_announces_no_success` | PolicyKit-refused create: signal for the recorded failure, reread shows no network | pass |
| `state_events::cr_f1_unchanged_observations_and_reads_announce_nothing` | unchanged samples and reads: zero signals | pass |
| `state_events::cr_f1_a_burst_coalesces_and_the_final_state_is_announced` | 200 changes → 1 signal; final state visible | pass |
| `state_events::cr_f1_an_emission_failure_keeps_the_change_pending_and_never_fails_the_mutation` | emission fails (interface removed); mutation still succeeds; pending signal delivered once emission works | pass |
| `watcher::cr_f1_a_real_daemon_change_reaches_the_production_watcher` | real `DaemonService` + publisher → production `watch` → `Changed`; ≈5 ms | pass |
| `client_link::every_link_phase_is_announced_even_when_health_does_not_change` | four phases marked, health moved twice | pass |
| `lib::authenticated_peer_arrival_and_departure_change_the_published_snapshot` | peer arrival/departure visible to the recheck | pass |

All signals were produced by real request dispatch or worker update paths; no test calls an emit
function. `state_events` passed three consecutive runs (≈3.6 s each). Checks at the fix: fmt,
clippy, workspace tests, all nine private-bus suites, desktop format/lint/typecheck/test/build.

**Not claimed:** radio-loss detection within one second (the 15 s hardware probe interval is
unchanged); authenticated peer arrival through a real TLS session is exercised only through the
snapshot it updates.

## CR-F6 — effect-generation resource ownership (2026-11-13)

**Fix:** `25e698475f14722d45801e38a81ad1c9f367a23f`. `apps/desktop/ui/src/useDaemon.ts`: the
shared `alive`/`inFlight`/`pending` refs are gone. Each effect run keeps `disposed`, `inFlight`,
`pending` and its `unsubscribe` as closure locals. A late `subscribe` result of a disposed run calls
its `stop` immediately (once) and is never stored; signal callbacks and read completions of a
disposed run return without touching link/view state or starting reads; cleanup stops the stored
subscription once and clears the shared refresh handler only if it is still this run's. StrictMode
remains on in `main.tsx`; no lint rule is disabled (`oxlint --deny-warnings` clean).

**Regressions** (`apps/desktop/ui/src/useDaemon.test.tsx`, jsdom; the probe renders
`phase:network:link`, so visible data is asserted with the counts):

| Test | Covers | With the fix | With the baseline hook |
| --- | --- | --- | --- |
| StrictMode replay … (in order) | UI-1: stops `[1, 0]` while mounted (one live listener), `[1, 1]` after unmount | pass | **fail** |
| StrictMode replay … (in reverse) | the other resolution order | pass | **fail** |
| a disposed generation's late signals and reads … | UI-2: replayed callback inert (no link change, no reads); current one works; nothing after unmount | pass | **fail** |
| changing api and subscribe … | UI-2/3: old subscription stopped; obsolete read finishing late cannot overwrite "Office"; old callback inert; new coalescing not blocked | pass | **fail** |
| reads pending at unmount … | completions after unmount start nothing | pass | pass |
| rapid remounts … | three mount/unmount cycles then a mount: one live, `[1, 1, 1]` released, zero after unmount | pass | pass |
| a rejected subscription … | page still loads; nothing to release | pass | pass |
| a burst of signals during a load … | 20 signals during a load → one follow-up read | pass | pass |

The baseline column was produced by restoring the committed hook, running the file (4 failed,
4 passed) and restoring the fix. Desktop checks at the fix: format, lint, typecheck, 43 tests,
build.

## CR-F3 — graph consistency (2026-11-13)

**Fix:** `4583c6ee23da604ac01ec1a21e50fb6872a8c9b4` (`crates/blueroute-core/src/topology.rs`;
contract in its doc comments and `docs/SPEC.md` under `blueroute-core`).

CR-F3-01 caller audit: every `add_segment`/`add_link`/`remove_link` caller is a test
(`topology.rs`, `forwarding_policy.rs`, `route_planner.rs`, `route_recovery.rs`, `neighbor.rs`,
`blueroute-linux` `forwarding.rs` and `forwarding_reconciliation.rs`); no production path builds the
graph yet, so the conservative rejection policy (GRAPH-2) breaks no caller. Semantics:

| Operation | Result |
| --- | --- |
| `add_segment`, new SegmentId | inserted, `Ok(true)` |
| `add_segment`, same SegmentId and NAP | kept with all members; new explicit members merged; `Ok(false)` |
| `add_segment`, same SegmentId, other NAP | `Err`, graph unchanged |
| `add_segment`, unknown NAP or explicit member | `Err`, graph unchanged |
| `add_link`, new LinkId | inserted, PANU becomes a member, `Ok(true)` |
| `add_link`, same LinkId, segment and endpoints | state/health/cost updated, `Ok(false)` |
| `add_link`, same LinkId, other segment/NAP/PANU | `Err` (no rebinding), graph unchanged |
| `add_link`, unknown endpoint/segment, NAP ≠ segment NAP, NAP = PANU | `Err`, graph unchanged |
| `remove_link` | PANU stays a member while another link in the segment needs it or it is explicit |
| `remove_node` | its links, segments it is NAP of, and its memberships (explicit too) go |

Explicit members (`PanSegment::add_member` before insertion) are supported, must be graph nodes,
and are tracked separately from link membership.

**Regressions** (`cargo test -p blueroute-core --lib cr_f3`, 11 tests, all pass): the four
baseline tests (identical reinsertion, NAP change, LinkId rebinding, whole-graph consistency),
now enabled; plus property update (and its routing effect: a failed link drops out of
`shortest_path`, recovers in place), refused moves to another segment or swapped roles with
other invalid changes (whole-graph equality), a member with two links, removal after an update,
explicit members (unknown refused atomically; survive link removal; go with the node), return
values, and an unchanged deterministic routed path across refused changes. A shared
`assert_consistent` checks GRAPH-3 after each. Existing single-star, routed, redundant-path,
partition, forwarding and neighbor tests are unchanged and pass (workspace tests green).

## CR-F4 — fresh hosting capability (2026-11-13)

**Fix:** `6025bbfcce6f4ff22948743545b252a6011994f3`.

- `NetworkOperations::observe_capabilities` (default no-op). `SingleStarNetworkOperations` keeps
  the observation in a `Mutex` slot seeded with the startup probe; `create_network` and
  `reconcile_on_startup` check `current_capabilities()` at the operation boundary (a poisoned
  slot refuses).
- `DaemonService::update_runtime_observation` passes each observation to the operations while
  still holding the status lock, then marks the CR-F1 publisher after releasing it, so observers
  reread a status that agrees with the gate (CR-F4-06).
- Freshness and failure: the runtime worker reprobes every 15 s; a failed probe publishes
  `NodeCapabilities::default()` (unknown), which refuses creation. Backend checks at operation
  time and rollback on backend failure are unchanged; recording an observation does nothing else.
- CR-F4-02 latched-copy review: `spawn_daemon_control_worker_with_peers` and `JoinControlConfig`
  keep the startup capabilities for (a) `connection_policy_ceiling`, configured policy (sourced
  from `DaemonConfig`), and (b) the `Hello` self-description, which no current receiver uses for
  a decision (`topology_plan`/`topology_policy` consumers are not wired into the daemon). Both are
  documented as deliberately latched in `docs/SPEC.md` §13. Membership, pins, ceilings,
  authorization and the closed join gate are untouched by observations.

**Regressions** (`cargo test -p blueroute-daemon --lib cr_f4`, 5 tests, all pass; one service
instance, fake runtime, no restart):

| Test | Covers |
| --- | --- |
| `cr_f4_recovered_hosting_capability_unblocks_create_without_restart` | false → true; create reaches the runtime (baseline **fail**: `NotSupported` while status said `Some(true)`) |
| `cr_f4_unknown_to_true_recovery_unblocks_create` | unknown → true |
| `cr_f4_lost_or_failed_observations_close_the_gate_again` | true → false and true → probe failure: `NotSupported`, runtime never reached, no network |
| `cr_f4_reading_or_refreshing_capabilities_creates_nothing` | observation plus `GetCapabilities`/`GetStatus`/`ListNetworks` start nothing |
| `cr_f4_concurrent_observations_leave_status_and_gate_in_agreement` | 8 threads × 200 alternating observations; create succeeds iff status shows hosting |

Checks at the fix: fmt, clippy, workspace tests, `state_events`, `authorization` bus suites.
Hardware adapter-reset acceptance remains separate (CR-QUALIFY).

## CR-F5 — removal cleanup, software (2026-11-13)

**Design (CR-F5-01):** [CR-F5-REMOVAL-CLEANUP.md](CR-F5-REMOVAL-CLEANUP.md): entry point, scope,
lock/marker quiescence, order, 60 s bound, failure and Debian behavior. **Fix:**
`0b0beabae3a83c20fbd759fa307a25189dd4f8a3`.

| Subtask | Where | Evidence |
| --- | --- | --- |
| 02 stopped-service cleanup, typed adapters, no new frontend privilege | `apps/daemon/src/removal.rs`, `main.rs` `--remove-owned-networking`; `SystemClientLeave` (NetworkManager/BlueZ D-Bus) | root-only (`/proc/self` owner), single fixed flag, not on the D-Bus API; `maintenance_mode.rs` (4 tests: other arguments → 2, non-root → 2 before touching state) |
| 03 no writer during removal | instance lock `daemon.lock` (daemon holds it for life), `removal-pending` marker (daemon refuses to start) | `a_running_daemon_blocks_cleanup_and_a_pending_removal_blocks_the_daemon`; binary tests: marker → exit 1 "being removed", held lock → exit 1 |
| 04 owned resources only | `remove_known_networks` over `leave_observed`; scope = registry NetworkIds + client record | `owned_state_of_known_networks_goes_and_everything_else_stays` (foreign preserved, counted unresolved); `a_foreign_panu_link_is_left_alone` |
| 05 forwarding | none: the daemon never writes forwarding (no lease exists); cleanup never touches it | design table; code search: no forwarding writer in `apps/daemon` |
| 06 NetworkManager-only, partial failure | Bluetooth optional; per-network failures do not stop others; reobservation decides | `a_proven_owned_panu_link_is_disconnected_first_and_only_with_bluetooth` (both cases), `a_partial_failure_reports_what_remains_and_keeps_going` (membership untouched, retry succeeds) |
| 07 Debian paths | `prerm` remove/deconfigure only; `postinst` configure/abort-remove/abort-deconfigure clear marker; `postrm` purge deletes state | `test-maintainer-lifecycle.sh` (install, upgrade, remove, deconfigure, both aborts, direct purge order, purge after remove) |
| 08 failure not swallowed | cleanup exit status propagates through `set -e`; message tells the administrator to fix and retry | lifecycle test: failing cleanup fails `prerm`, state kept; real dpkg: removal fails, package stays installed, `abort-remove` restarts the service, retry succeeds |
| 09 fake-backend matrix | `removal.rs` tests (10) | active/absent (repeat no-op)/foreign/PANU proof, concurrent writer (lock), service absence (unreachable, no known network → no contact), timeout, partial failure, blind inventory (error, not success), unreadable registry (nothing removed) |
| 10 isolated dpkg | `test-package-upgrade.sh` | real `dpkg --remove` with failing then succeeding cleanup, exact call order `stop → cleanup → disable`, purge, v2→v3 upgrade kept; `rm` guard on for every operation |
| 11 packaged artifacts | CI extracts the built `.deb`, runs the lifecycle test on its embedded scripts, checks `/usr/libexec/blueroute/blueroute-daemon` is executable before the dpkg test swaps it for a mock | local run of both tests on `blueroute_0.1.0_amd64.deb` passed |
| 12 installed-system qualification | — | **open**, CR-QUALIFY |

Scripts prefix every fixed path with `${DPKG_ROOT:-}` (empty in normal operation; set by dpkg for
chrootless runs), and the lifecycle test fails if any fixed path lacks it.

**Not claimed:** that no active networking remains after uninstall on a real system; that a PANU
link of a joined client ends with the daemon (BlueZ is expected to drop it with the caller, but
cleanup does not rely on it). Both need CR-QUALIFY.

## CR-F7 — documentation reconciliation (2026-11-13)

Facts rechecked against source and the built packages (DOC-1): API **1.7** (`ApiVersion::CURRENT`;
1.6 at the reviewed baseline, raised by CR-F1), membership schema **v3**
(`CURRENT_FORMAT_VERSION = 3`), `blueroute_0.1.0_amd64.deb` ships `/usr/bin/blueroute`,
`/usr/bin/blueroute-tui` and `/usr/libexec/blueroute/blueroute-daemon`, and
`blueroute-desktop_0.1.0_amd64.deb` ships `/usr/bin/blueroute-desktop`.

| Subtask | Change |
| --- | --- |
| 01 | `SPEC.md` current status: API 1.7, schema v3, package split (the "daemon and CLI only" line replaced), status date 2026-11-13. |
| 02 | `ADR-INDEX.md`: P8-009 indexed; P7-001 status current; status words defined (accepted ADR / implemented / qualified); a separate table of accepted design records (P6-004, P6-009, P8-005, CR-F5). |
| 03 | Superseded labels, history kept: `TODO.md` (block under the 2026-10-16 status date; P6-004, P7-003, P7-004, P15-003 rows), `P7-001-CONTROL-TRANSPORT.md`, `P6-004-JOIN-ORCHESTRATION.md` addendum status, `P14-001-THREAT-MODEL.md` release gate; `DEVELOPMENT.md` placeholder-crate sentence replaced. |
| 04 | Guarantees documented where they are accepted: owner binding and owner-loss (`SPEC.md` client bullet), `StateChanged` publication (SPEC §11, `P5-PROTOCOL.md` API 1.7), capability freshness (SPEC §13, `TROUBLESHOOTING.md`), graph contract (SPEC `blueroute-core`), removal/upgrade/purge (SPEC §31, `ADMINISTRATOR.md`, `USER_GUIDE.md`, `CR-F5-REMOVAL-CLEANUP.md`). |
| 05 | Preserved: historical evidence and task IDs, the read-only TUI, closed join, absent live routing, disabled sharing (every new text restates the gate rather than relaxing it). |
| 06 | "Code review (2026-11-13)" notes on P2-001..P2-008, P5-005, P6-001, P6-004, P7-003, P7-004, P15-003, P15-005 rows and the P11-001, P12-002, P17-003, P17-006 sections; no checkbox changed. |
| 07 | Relative-link check over `docs/*.md`, `README.md`, `CONTRIBUTING.md`, `CLAUDE.md`: 0 missing files or anchors. Durable records cite repository tests and commits, not temporary artifacts. CR-CLOSEOUT repeats the comparison at the final revision. |

Note: CR-VALIDATE-06 names "Ralph Bridge" for CI inspection; this remediation inspects exact-head
CI with the GitHub CLI and records run IDs.

## CR-INTEGRATION — corrected boundaries end to end (2026-11-13)

**Commit:** `d1d86a5` (plus the integration tests already added with each fix).

| Subtask | Tests (all private bus or fakes) |
| --- | --- |
| 01 actual watchers | `blueroute-desktop` `watch_integration` runs the unchanged `watch_daemon` (re-running its test binary with `DBUS_SYSTEM_BUS_ADDRESS` = the private bus): change signal → `Changed`, idle loss → `Disconnected` within 2 s, API+1 daemon never connects, compatible return reconnects on the production backoff. TUI `tui_watcher_follows_changes_idle_loss_return_and_quit` drives the real watcher thread/channel (`spawn_with`, production `spawn` passes `BlueRouteClient::connect`) through change, idle loss, return and quit (its bus connection is released). Shared `watcher` suite: replacement, incompatible return, backoff schedule, consumer stop. `owner_race`: queued replacement events and the controlled owner races. |
| 02 real producers, second observer | `state_events`: requests from one connection (create/leave, rename, refused create) and worker publications reach two independent observers, which reread; nothing emits by hand. `watcher::cr_f1_a_real_daemon_change_reaches_the_production_watcher`. |
| 03 races, bursts, failure, link phase | subscription-setup race (`owner_race::cr_f2_an_owner_change_during_subscription_setup_is_not_missed`), reread-on-connect (desktop "rereads once the live subscription is up", TUI link test), 200-change burst, emission failure after commit, client-link phases with unchanged health (`client_link::every_link_phase_is_announced_even_when_health_does_not_change`). |
| 04 capability recovery scenario | `cr_f4_recovered_hosting_capability_unblocks_create_without_restart`: production `SingleStarNetworkOperations` + fake runtime + `DaemonService` status, unavailable at startup; recovery published to observers (publisher marked) and creation reaches the runtime. |
| 05 graph consumers | `cr_f3_refused_changes_leave_paths_deterministic` (routing), `forwarding_policy::cr_f3_graph_updates_reach_the_forwarding_decision_and_refused_ones_do_not`; whole-graph before/after equality on every refused change. |
| 06 cleanup ordering and lifecycle | `removal.rs` (owned and foreign, PANU first, partial failure then retry), lifecycle test (`stop → cleanup`, failure → `abort-remove`), real dpkg remove failing then succeeding. |
| 07 CI and isolation | Every new bus suite has a CI step (`owner_race`, `owner_watch`, `watcher`, `state_events`, `watch_integration`, `tui_watcher`); exact invocations in `CLAUDE.md`. No test mutates host networking: buses are `dbus-run-session` private buses, state is in temporary directories, the daemon binary tests are refused before any bus, and the dpkg test's `rm` guard rejects any path outside its isolated root. |

**Defect restoration** (each fix reverted in the working tree, the tests run, the fix restored):

| Restored defect | How | Result |
| --- | --- | --- |
| F1 | publisher never emits | `state_events` 6 of 7 fail (the silence test passes, as it should); daemon-to-watcher test fails |
| F2 | owner change does not end a subscription | desktop `watch_integration`, TUI watcher, shared `watcher` (3 of 6) and `owner_watch` fail |
| F3 | baseline overwrite in `add_segment`/`add_link` | 7 of 12 `cr_f3` tests fail, including the forwarding consumer |
| F4 | operations ignore new observations | 4 of 5 `cr_f4` tests fail |
| F5 | `prerm` cleanup with `|| true` | lifecycle test fails ("prerm remove must fail when cleanup fails") |
| F6 | baseline `useDaemon` (CR-F6 record) | 4 of 8 hook tests fail |
| F8 | baseline client (CR-F8 record) | 2 of 5 `owner_race` tests fail |

Full local run at `d1d86a5`: fmt, check, clippy clean; 888 workspace tests pass (31 ignored are
the bus suites, all run separately and passing); desktop 43 tests and build pass.

## CR-CLOSEOUT-01 — independent review of 4ca5042..HEAD (2026-11-13)

An independent review read the whole remediation diff against the specification (lock order,
emission off locks, publisher shutdown, owner races, instance lock lifetime, maintainer-script
paths, cleanup scope, graph atomicity, effect ownership) and found one real defect:

- **Failed reads caused a reread loop (fixed).** A failed read added a timestamped entry to recent
  failures, which was part of the compared snapshot, and also marked the publisher. Every failed
  read therefore emitted `StateChanged`, every front end reread, and a read that keeps failing
  (unreadable membership store, discovery after bluetoothd stops) looped. Any local user could
  also force broadcasts. Fixed in the next commit: recent failures are no longer compared and a
  failed read marks nothing; failed mutations are still announced (every mutating request
  publishes). Regression `state_events::cr_f1_failing_reads_do_not_make_observers_reread_in_a_loop`
  failed before the fix ("a failed read was announced") and passes after. This supersedes the CR-F1
  inventory row "recent failures … `observed()` for failed reads".

Accepted low-severity limitations, documented rather than changed:

- A daemon restart in the instant between subscribing to owner changes and resolving the owner can
  queue a stale notice, ending a fresh subscription with `OwnerLost`; the watcher reconnects after
  1 s. Fail-safe (already noted under CR-F2).
- The publisher's baseline is taken just after the daemon starts serving; a change within those
  microseconds before the baseline could go unannounced to a client that read before it. Startup
  only.
- `DPKG_ROOT` relocates paths only. A chrootless dpkg run (not normal operation) would still run
  cleanup against the host's buses; it touches only networks recorded in that root's state.

No issue was found in client owner binding, the shared watcher, lock ordering, capability
freshness, removal scope and maintainer scripts, graph atomicity, or `useDaemon` ownership.

## CR-VALIDATE — software candidate (2026-11-13)

**Candidate:** `42ae7abe77607663bc1fb208698723aaac3ee4d2` (the review fix on top of
`259380f`, whose exact-head [CI 43518870135](https://github.com/ekkus93/blueroute/actions/runs/43518870135) passed). Rows 02 and 04 ran at `259380f`;
the fix touches only `apps/daemon/src/lib.rs` and a daemon bus test, so they are unaffected. Toolchain 1.95.0, Node 26.13.0, npm 11.20.0.

| Subtask | Commands | Result |
| --- | --- | --- |
| 01 | `cargo fmt --all -- --check`; `cargo check --workspace --locked`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo test --workspace --locked` | clean; clean; clean; 888 passed, 0 failed, 32 ignored (the bus suites, run below) |
| 02 | `cargo test -p blueroute-daemon --features experimental-join --locked`; default build `join_network::tests` (incl. `default_builds_cannot_open_the_join_gate`) | 369 passed; 37 passed. CI and the package build without features. |
| 03 | `dbus-run-session -- cargo test …` for `dbus_service`, `authorization`, `dbus_client`, `cli_exit_contract`, `bridge_end_to_end`, `owner_race`, `owner_watch`, `watcher`, `state_events`, `watch_integration`, TUI `tui_watcher`; `python3 scripts/test_p6_006_loopback.py`, `scripts/test_p6_006_socket_traffic.py` | 1, 1, 1, 1, 1, 6, 3, 6, 8, 2, 1 passed; Python: TCP/UDP PASS, 5 unit tests OK |
| 04 | `npm ci`; `npm run format:check`; `npm run lint`; `npm run typecheck`; `npm test`; `npm run build` | all pass; 43 tests (8 `useDaemon` incl. StrictMode). No theme tokens changed (text only), so no contrast change to validate. |
| 05 | `bash packaging/debian/build-package.sh`; `dpkg-deb --field/--contents`; packaged-script `test-maintainer-lifecycle.sh`; `test-package-upgrade.sh` | `blueroute_0.1.0_amd64.deb` sha256 `33fc67f27941cc6f6e362ffb6fd2ef41abac3d3fed7859afdc0c8b55a1da0615` (Depends: dbus, bluez, network-manager, polkitd, systemd); `blueroute-desktop_0.1.0_amd64.deb` sha256 `a656333cf30190842516c4a2f8040fca966c4c4fac2bd8d3f514b1348e86b0f2` (Depends `blueroute (= 0.1.0)`, WebKitGTK 4.1 and shlibs); build verified every built frontend asset is embedded, and the bundle carries the new stale-data text; lifecycle and isolated dpkg upgrade/migration/remove-failure/remove/purge tests pass. Hashes are of this local build; package builds are not bit-reproducible. |
| 06 | exact-head CI | [CI 43520783589](https://github.com/ekkus93/blueroute/actions/runs/43520783589) passed at `42ae7ab` (Rust and Desktop frontend jobs, including every new bus-suite step) |
| 07 | invariant regressions inside the 888 passing tests, by name: authorization 27, pin 57, exporter 6, revocation 8, rollback 3, foreign 28, identity 29, migration 3, replay 8, sharing 3 | all pass |

**Limitations:** nothing here is installed-system or hardware evidence. CR-F5 and CR-F4's
hardware claims stay with CR-QUALIFY.


## CR-CLOSEOUT — software milestone (2026-11-13)

- **01 review:** each fix was reviewed against its requirements while it was accepted (entries
  above), and the whole diff was reviewed independently (CR-CLOSEOUT-01 entry); its one defect is
  fixed at `42ae7ab`.
- **02 linkage:** every finding has a fix commit, regressions that fail when the defect is
  restored (CR-INTEGRATION), and exact-head CI: run 43518870135 at `259380f` covers F1–F8 and the
  integration suites; [CI 43520783589](https://github.com/ekkus93/blueroute/actions/runs/43520783589) passed at `42ae7ab` (Rust and Desktop frontend jobs, including every new bus-suite step) covers the review fix.
- **03 original tasks:** the 8 original status rows touched by CR-F7 keep their checkbox states
  (checked mechanically against the baseline); only "Code review" notes were added. Physical,
  join, routing, support and release criteria remain open as before.
- **04:** not complete. CR-QUALIFY (installed-system and Bluetooth-rig evidence) has not run, so
  CR-F5 stays SOFTWARE COMPLETE / QUALIFICATION DEFERRED and the remediation is a **software
  milestone**, not COMPLETE. Nothing here implies completion of the wider product roadmap.

## Correction (2026-11-13, code-review follow-up)

The CR-CLOSEOUT-01 entry above accepted "a change within those microseconds before the baseline
could go unannounced" as a startup-only limitation. That gap (GAP-01) is now closed by FU-05: the
publisher announces once at start and once at its first network sample. Further follow-up
findings reopened the checked CR-F1, CR-F2, CR-F3 and CR-F5 claims; see
[CODE_REVIEW_FOLLOWUP_EVIDENCE.md](CODE_REVIEW_FOLLOWUP_EVIDENCE.md). The entries above are kept as
the historical record of what was verified at the time.

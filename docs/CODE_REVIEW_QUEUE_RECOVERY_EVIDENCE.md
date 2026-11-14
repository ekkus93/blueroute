# Code Review Queue and Recovery Remediation Evidence

Versioned evidence for [CODE_REVIEW_QUEUE_RECOVERY_TODO.md](CODE_REVIEW_QUEUE_RECOVERY_TODO.md)
(specification: [CODE_REVIEW_QUEUE_RECOVERY_SPEC.md](CODE_REVIEW_QUEUE_RECOVERY_SPEC.md)). Software
results only; nothing here is installed-system or physical qualification.

## QR-BASE — Findings preserved and corrected expectations established (2026-11-14)

**Environment (QR-BASE-01).** Source `e7720a9` (the two queue/recovery documents only, on top
of the reviewed baseline `f957d80207930ccb481bec8f23cefd8bb2506cde`; no code difference). Clean
tree apart from the never-committed `.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, Node 26.13.0,
npm 11.20.0, Python 3.13.5, Debian 13 kernel 6.12.115, dbus-daemon 1.16.2, zbus 5.19.0 with
async-broadcast 0.7.2. Private brokers via `dbus-run-session`; test temporary state under the
ignored workspace `tmp/` (`.cargo/config.toml` sets `TMPDIR`). Guidance read: `CLAUDE.md`,
`docs/SPEC.md`, the lifecycle specification/backlog/evidence, this specification and backlog,
`docs/ADMINISTRATOR.md`, `docs/CR-F5-REMOVAL-CLEANUP.md`, `docs/CR-QUALIFY-PROCEDURE.md`.

### Pinned transport facts the analysis relies on (QR-BASE-02)

Read from the pinned zbus 5.19.0 sources:

- `connection/socket_reader.rs`, `receive_msg`: the single reader reads one message, dispatches a
  method reply or error to its waiting caller, then `broadcast_direct(..).await`s it into every
  matching stream before it reads the next message. A full matching stream therefore stops all
  reading on that connection, including replies to unrelated calls.
- `connection/mod.rs`, `add_match`: every match stream gets a broadcast channel of capacity
  `DEFAULT_MAX_QUEUED` (64) unless asked otherwise, created with `set_await_active(false)`.
  Subscriptions with the **same match rule share one channel** (`activate_cloned`), so an active
  receiver that is not read (for example in a retained handle) holds back every subscriber of that
  rule.
- Consequence: by the time a reply is in hand, every earlier message is queued in its streams
  (the LC-OWNER ordering argument still holds), but a reply can only be read while every matching
  stream has room for the messages ahead of it.

### RLC-01 — ownership-queue saturation blocks final verification

- **Trigger.** A subscription bound to owner A sees A emit an item. A answers the item's Version
  check and then releases and reacquires the name N times (2N ownership notices). The consumer is
  paused from the Version call until all of that has completed, then resumes.
- **Affected path.** `crates/blueroute-client/src/lib.rs`, `EventSubscription::next_change`. The
  Version future is ready on resumption, so `future::or` returns it without polling the ownership
  branch; the final `current_owner` (`GetNameOwner`) then waits for a reply queued behind
  notices that nothing reads while the ownership stream is full. Nothing else drains it, so the
  call never returns.
- **Earlier tasks.** LC-OWNER-02/05/06 and LC-INTEGRATION-01 (EPOCH-2/4): their tests used at most
  four notices, below the queue capacity.
- **Signal selection (QUEUE-1).** The same circular wait exists for the owner's own signals: the
  Event and StateChanged streams are not read while a verification reply is awaited, so more
  signals than the reader can hold ahead of the reply stall the subscription.
- **Corrected expectations.** `crates/blueroute-client/tests/queue_saturation.rs` (ignored,
  private bus; not yet scheduled in CI). Ordering is established on the daemon side: each
  release and reacquire is the fake daemon's own broker round trip, so the broker has queued all
  notices for the client before the burst is marked complete; nothing depends on the client's
  possibly blocked reader. Bursts are finite; the fake daemon's own unfiltered queue is enlarged
  so the fixture never stalls itself.
- **Baseline result** (`dbus-run-session -- cargo test -p blueroute-client --test
  queue_saturation --locked -- --ignored` at `e7720a9`, 3 runs, identical each time): 3 passed,
  6 failed.

  | Test | Burst | Result at baseline |
  | --- | --- | --- |
  | `qr_queue_a_burst_below_capacity_retires_event` / `_state` | 20 cycles, 40 notices | pass (control) |
  | `qr_queue_a_burst_at_capacity_retires_event` | 32 cycles, 64 notices | pass (boundary control) |
  | `qr_queue_a_burst_just_above_capacity_retires_state` | 33 cycles, 66 notices | *"no OwnerLost within 2s of resuming: None"* |
  | `qr_queue_forty_cycles_after_the_reply_retire_event` / `_state` | 40 cycles, 80 notices | *"no OwnerLost within 2s of resuming: None"* |
  | `qr_queue_forty_cycles_then_drop_keep_the_connection_working` | 40 cycles | same; fails before the drop |
  | `qr_queue_signals_queued_ahead_of_a_reply_are_all_delivered` | 96 signals | *"stalled after 0 items"* |
  | `qr_queue_a_signal_flood_ends_the_subscription_without_stalling` | 320 signals | *"stalled after 0 items"* |

  The threshold sits exactly at the queue capacity: 64 notices pass and 66 fail. That is the
  failure the review describes, explained by the reader, not by timing.
- **Scope.** One private broker, fake daemon; Event and StateChanged subscriptions. No physical
  or installed-system claim.

### RLC-02 — the documented manual recovery leaves an unresolved client record

- **Trigger.** Member/None for network N plus a client record naming N (the RFU-02 state). The
  administrator confirms there is no PAN link and deletes N's owned NetworkManager state, then
  follows step 4 of `docs/ADMINISTRATOR.md` "Removal with an unresolved role": for a `member`
  network with no role it says to delete the `network` and `peer` lines only.
- **Affected path.** `docs/ADMINISTRATOR.md` step 4. The retained record names a network with no
  membership, which `removal_scope` correctly classifies as OrphanClientRecord, so removal still
  exits 1. The cleanup classifier is right (it refuses uncertainty); the instructions are
  incomplete. `docs/CR-QUALIFY-PROCEDURE.md` A3 "Unknown active role, recovery" already deletes
  both, so only the guide disagrees.
- **Earlier tasks.** LC-CLEANUP-07 (recovery by restoring a verified role, a different path) and
  LC-CLOSEOUT (documentation review).
- **Corrected expectation.** `qr_recovery_the_documented_recovery_of_an_unknown_role_completes`
  in `apps/daemon/src/removal.rs`: production stores, a fake backend standing in for the
  independently completed NetworkManager cleanup, and a helper that edits `memberships-v1`
  exactly as the guide's step 4 instructs. A foreign network's state must stay reported.
- **Baseline result** (`cargo test -p blueroute-daemon --lib --locked qr_recovery`): fails with
  *"the documented recovery left removal failing: RemovalReport { …, unresolved: 2,
  unresolved_links: 0, unresolved_roles: 1, … }"*. Like LC-BASE-04, this unit test runs in the
  ordinary suite, so it is committed with its correction (QR-RECOVERY) rather than left failing
  in CI.

### Isolation (QR-BASE-04)

- Broker tests use only the `dbus-run-session` private session bus; the client and fake daemon
  are session connections. No system bus, BlueZ, NetworkManager or package database is reached.
- Bursts and floods are finite (at most 40 cycles, 320 signals); every wait on the client is
  bounded by 2 seconds; daemon-side waits poll flags the test sets.
- Removal tests use the fake `ClientLeaveBackend` and production stores in a `tempfile` directory
  under the ignored workspace `tmp/`, removed on drop.

## QR-QUEUE — Bounded-queue progress with ordered retirement (2026-11-14)

**Source.** `crates/blueroute-client/src/lib.rs` (`EventSubscription`, `step`, `keep_reading`,
`settle`, `decide`, `ClientError::SubscriptionLagged`, `SUBSCRIPTION_BACKLOG`); contract in
`docs/SPEC.md` (client paragraph) and the `EventSubscription` documentation.

### Queues, capacities and progress argument (QR-QUEUE-01, QUEUE-1)

| Queue | Owner | Capacity | Read while |
| --- | --- | --- | --- |
| `Event` signals of the bound owner | zbus match stream, shared per rule | 64 | every await of `next_change` |
| `StateChanged` signals (`changes()` only) | zbus match stream, shared per rule | 64 | every await of `next_change` |
| Ownership notices of the daemon's name | zbus match stream, shared per rule | 64 | every await of `next_change` |
| Undelivered signals | subscription backlog, receive order | `SUBSCRIPTION_BACKLOG` = 256 | — |

- Every await inside `next_change` (selecting a signal, the item's Version reply, the final
  `GetNameOwner` reply) is raced against `keep_reading`, which reads all three streams. So no
  awaited reply can sit behind this subscription's own unread messages: as long as the call is
  polled, the reader can always reach the reply.
- Between the Version reply and the final call, `settle` reads everything already queued
  without waiting. Every notice it finds was received before the final check was sent, so a
  period end there is pre-decision and withholds the item (no extra round trip is needed to
  decide it). This is the step that removes the RLC-01 circular wait: a ready Version reply no
  longer leads straight into an await behind a full ownership queue.
- While the final reply is pending, a period end does **not** interrupt: its position is kept in
  `period_end` on the subscription and compared with the reply's position by `decide` (QUEUE-3).
- Signals read during an await join the backlog; beyond 256 the subscription ends with
  `SubscriptionLagged { limit: 256 }` (Operational) instead of stalling the connection. The one
  production consumer, the shared watcher, treats any error as a loss: it reports Disconnected,
  reconnects with its bounded backoff and rereads.
- Bounded work: reading is driven only by the caller's own poll (no task is spawned, nothing
  is detached, nothing busy-polls); each step handles one message; memory is bounded by the
  backlog.
- Outside a call nothing is read: a caller that stops polling can stop its connection's reader
  until it resumes. QUEUE-2 does not require progress while the caller is paused; on resumption
  the bound applies.
- **Pinned dependency assumptions.** One reader per connection, replies dispatched before the
  reader waits for room, 64-entry match channels shared per rule (zbus 5.19.0, async-broadcast
  0.7.2; see QR-BASE). `qr_queue_the_transport_argument_is_pinned_to_the_verified_zbus` (ordinary
  test, runs in CI) fails if either version changes in `Cargo.lock`, so an upgrade cannot
  silently invalidate the argument.

### Delivery decision and terminal state (QR-QUEUE-02/03/04)

- Pre-decision period ends withhold the item (`settle` before the final call, and `decide` for
  notices read during it); post-decision ones keep the permitted delivery and retire the next call
  (`Decision::DeliverThenRetire`; `period_end` stays recorded). A failed final check retires only on
  NameHasNoOwner or an already recorded period end; a transient failure is reported as itself.
  Returning owners still need a fresh subscription.
- The item being checked stays first in the backlog until it is delivered, and `period_end` lives
  on the subscription: cancelling a call loses neither the item nor a recorded loss (this also
  removes the earlier documented loss of the in-flight item on cancellation).
- `end()` drops all three streams and the backlog on retirement and on lag, so a retained handle
  holds nothing back; dropping releases its share of the rules without affecting other
  subscribers of the same rules.

### Regressions (QR-QUEUE-05/06/07/08)

`dbus-run-session -- cargo test -p blueroute-client --test queue_saturation --locked -- --ignored`:
13 passed, 10 consecutive runs. Against QR-BASE, 4 tests were added (cancellation, shared-rule
survival, StateChanged bursts and flood).

| Test | Covers |
| --- | --- |
| below/at/just-above capacity, Event and State (20, 32, 33 cycles) | QUEUE-5 boundary matrix |
| `qr_queue_forty_cycles_after_the_reply_retire_event` / `_state` | the RLC-01 reproduction; OwnerLost within 2 s of resuming, kept for later calls, second call on the connection with the retired handle retained, fresh subscription delivers |
| `qr_queue_forty_cycles_then_drop_keep_the_connection_working` | second call after drop |
| `qr_queue_a_call_cancelled_during_the_burst_loses_nothing` | cancellation and resumption |
| `qr_queue_a_live_subscription_survives_a_retired_one_on_the_same_rules` | unrelated active subscription on shared rules, before and after the retired handle is dropped |
| `qr_queue_events_` / `state_changes_queued_ahead_of_a_reply_are_all_delivered` (96 signals) | signal selection: every item delivered, each within 2 s |
| `qr_queue_an_event_flood_` / `a_state_change_flood_ends_the_subscription_without_stalling` (384 signals) | typed overflow, connection keeps working |
| `qr_queue_the_transport_argument_is_pinned_to_the_verified_zbus` (ordinary test) | QUEUE-6 dependency pin |

Unit tests (`cargo test -p blueroute-client --lib --locked`, 6 passed):
`qr_queue_a_period_end_before_the_decision_withholds_and_one_after_retires_next` is the
focused seam for the post-decision window, which no broker places deterministically (QUEUE-6 /
QR-QUEUE-07): before the reply withholds, after it keeps the delivery and retires the next call.
`qr_queue_an_overflow_is_typed_operational_and_names_its_limit`.

**Proof boundaries.** The real-broker tests prove transport progress, pre-decision withholding,
cancellation and shared-rule release on dbus-daemon 1.16.2 with zbus 5.19.0. The post-decision
ordering is proven by the `decide` seam and the code path that keeps `period_end`; it is not
observed on a broker.

**Preserved regressions (QR-QUEUE-08)**, at the QR-QUEUE change alone (the uncommitted RLC-02
recipe set aside):
- fmt clean; strict Clippy clean.
- `cargo test --workspace --locked`: 914 passed, 0 failed, 77 ignored.
- `cargo test -p blueroute-daemon --features experimental-join --locked`: 386 passed.
- Private broker suites, all passed: `ownership_period` 12 (A→B→A, A→unowned→A, held reply,
  cancellation, incompatible successor with 0 Requests, healthy owner, transient-error and
  queued-loss cases), `owner_race` 6, `event_owner_race` 6, `owner_watch` 3, `watcher` 8,
  `dbus_client` 1, `cli_exit_contract` 1, `dbus_service` 1, `authorization` 1,
  `state_events` 8, `publisher_progress` 6, `watch_integration` 2, `watch_handover` 4,
  `bridge_end_to_end` 1, `tui_watcher` 3.

## QR-RECOVERY — Complete targeted manual recovery (2026-11-14)

**Source.** Documentation: `docs/ADMINISTRATOR.md` "Removal with an unresolved role" (steps
1–5 and a case table), `docs/CR-F5-REMOVAL-CLEANUP.md` (recovery paragraph),
`docs/CR-QUALIFY-PROCEDURE.md` (A3 recovery case and status line). Tests:
`apps/daemon/src/removal.rs`. **No production code changed** (QR-RECOVERY-05): no role-editing
API, no schema change, no automatic record deletion; the classifier is untouched.

### Guide corrections (QR-RECOVERY-01/02/06)

- Step 1 requires the **full** 32-digit NetworkId and warns that profile/device names show only 8.
- Step 2 stops (leaves everything) when a link's ownership cannot be established; an interface
  name or hint alone proves nothing.
- Step 4 (the RLC-02 defect) now removes the network's `network`/`peer` lines **and** the client
  record if it names the same NetworkId, says that deleting only the membership leaves an
  unresolved orphan record, and keeps another network's record.
- A table covers Member/None with a matching record, Member/None without one (or with another
  network's), an orphan record, and a completed leave (no manual step).
- **Additional finding during this work (step 5).** The guide's check "`unresolved_foreign_resources`
  unchanged" was blind in one case: when the recovered network is the last one the installation
  knows, `run_removal_cleanup` returns an all-zero report without inventorying NetworkManager
  (`known.is_empty()`), so an owned profile left behind by an incomplete step 3 would not show.
  The guide now says so and has the administrator check `user.data` of every profile with
  `nmcli` in that case. The code is unchanged (that early return is the existing design: an
  installation that knows no network owns no networking state); the Part A recovery case still
  knows `H`, so its count comparison is meaningful, and the procedure now says so.
- Service-stopped/writer-quiesced ordering and "step 3 before step 4" are kept.

### Regressions (QR-RECOVERY-03/04/05)

Production membership and client-record stores, fake NetworkManager/BlueZ backend, the
production scope → cleanup → `cleanup_outcome` helper result. `follow_the_documented_step_4`
edits `memberships-v1` and deletes `client-record-v1` exactly as the corrected step 4 says;
`delete_owned_state` stands in for the administrator's independent steps 2–3.

| Test | Result |
| --- | --- |
| `qr_recovery_the_documented_recovery_of_an_unknown_role_completes` | the RFU-02 state recovers: success, helper exit 0, both records gone, repeat succeeds; all-zero report because no network is left known |
| `qr_recovery_deleting_only_the_membership_leaves_an_orphan_record_and_fails` | incomplete sequence: OrphanClientRecord, `unresolved_roles` 1 and exit 1 with Bluetooth off and on, record kept; completing step 4 then succeeds |
| `qr_recovery_without_a_matching_record_keeps_another_networks_record_and_peers` | another known network's membership, role, 2 peers and its record survive; foreign count unchanged; repeat succeeds |
| `qr_recovery_an_owned_profile_left_behind_raises_the_foreign_count` | negative control: skipping step 3 shows as +2 foreign resources |
| `qr_recovery_an_existing_orphan_record_is_cleared_after_its_resources` | orphan record: proof kept until its resources are resolved, then cleared; success repeats |
| `qr_recovery_a_completed_leave_needs_no_manual_step` | NotMember with a stale record: succeeds without recovery, owned state removed |

- **Baseline sensitivity.** With step 4 as documented at `f957d80` (membership lines only), the
  first test fails with `unresolved_roles: 1` (QR-BASE); it passes with the corrected step.
- **Retained.** LC-CLEANUP-07 tests that restore a verified Host/Client role
  (`lc_cleanup_recovery_needs_the_role_established_and_then_completes`,
  `lc_cleanup_a_recovered_host_role_finishes_with_host_cleanup`) and every proof-retention test
  pass unchanged.

**Results** (QR-QUEUE committed, this change on top):
- fmt clean; strict Clippy clean.
- `cargo test -p blueroute-daemon --lib --locked removal`: 33 passed (27 before, plus 6 new).
- `cargo test --workspace --locked`: 920 passed, 0 failed, 77 ignored.
- `cargo test -p blueroute-daemon --features experimental-join --locked`: 392 passed.
- `bash packaging/debian/test-maintainer-lifecycle.sh`: passed.

**Limitations.** Software simulation of the manual workflow only: the administrator's
observation of PAN links and NetworkManager is modelled by the fake backend. Installed-system
execution remains QR-QUALIFY-02 / LC-QUALIFY-02 / CR-QUALIFY on a disposable VM.

## QR-INTEGRATION — Consumer and helper boundaries, sensitivity and CI (2026-11-14)

### Production consumers (QR-INTEGRATION-01)

| Test | Boundary | Scenario | Expectation |
| --- | --- | --- | --- |
| `qr_integration_the_shared_watcher_reports_a_saturated_loss` (`crates/blueroute-client/tests/queue_saturation.rs`) | production `blueroute_client::watch` | the exact RLC-01 recipe: the test blocks the watcher's executor thread from the item's Version call until the daemon has replied and run 40 release/reacquire cycles | `Connected, Disconnected` within 2 s of resuming, then `Connected` and `Changed` for the current period; no `Changed` from the old one |
| `tui_watcher_reports_a_saturated_loss_without_the_old_changes` (`apps/tui/src/watch.rs`) | the TUI's real watcher thread (`spawn_with`) | inside the check of A's change, A emits 100 StateChanged signals and runs 40 release/reacquire cycles before replying | `Disconnected` within 2 s with none of the 100 old changes, then `Connected` to the returning A and its new change |
| `the_desktop_watcher_reports_a_saturated_loss_without_the_old_changes` (`apps/desktop/tests/watch_handover.rs`) | the desktop's real `watch_daemon`, with the private bus as the system bus | same flood as the TUI case | `Connected, Disconnected, Connected, Changed`, nothing else |

The adapters run their own watcher threads, which cannot be paused across a reply, so their
scenario is ordered entirely by the daemon's socket (flood, then cycles, then the reply). That
makes it deterministic: at baseline the full signal queue stops the reader before any ownership
notice is read, so the watcher reports nothing. The after-reply recipe is covered at the shared
watcher, which the adapters wrap.

### Recovery through the helper and package boundaries (QR-INTEGRATION-02)

- **Production stores → scope → cleanup → helper outcome.** The QR-RECOVERY tests call
  `run_removal_cleanup` and `cleanup_outcome`, the same functions the
  `--remove-owned-networking` helper in `apps/daemon/src/main.rs` uses, with the production
  membership and client-record stores. **Mocked:** the NetworkManager/BlueZ backend (a fake
  `ClientLeaveBackend`/`OwnedInventory`) and the root check (the real helper refuses non-root and
  talks to the system services, so it cannot run in CI).
- **Maintainer scripts.** `packaging/debian/test-maintainer-lifecycle.sh` now walks the manual
  recovery: an unresolved role fails remove, deconfigure and a direct purge at `prerm`; the
  incomplete recovery (membership only) still fails both, with abort paths restarting the
  service, and keeps the client record and identity; the complete recovery lets remove and purge
  finish and deletes the state directory. **Mocked:** the installed helper (its exit status is set
  per step to what the production helper returns for that state, citing the unit test that proves
  it) and the systemd helpers; the scripts are the real ones, under `DPKG_ROOT`.

### Negative controls (QR-INTEGRATION-03)

`tmp/neg-controls.sh` (ignored workspace directory) creates an isolated worktree under `tmp/`
with its own `CARGO_TARGET_DIR`, restores one old behavior at a time in that worktree only,
runs the tests that must catch it, and removes the worktree; tracked files are never modified.
Run at `128d44e`:

| Control | Restored behavior | Tests that fail |
| --- | --- | --- |
| C1 | old final await: no settle before it, nothing read during `GetNameOwner` | `qr_queue_a_burst_just_above_capacity_retires_state`, `qr_queue_forty_cycles_after_the_reply_retire_event`, `_retire_state`, `qr_queue_forty_cycles_then_drop_keep_the_connection_working`, `qr_queue_a_live_subscription_survives_a_retired_one_on_the_same_rules`, `qr_integration_the_shared_watcher_reports_a_saturated_loss` (6 of 14; the below/at-capacity controls pass) |
| C2 | retirement keeps its streams | `qr_queue_forty_cycles_after_the_reply_retire_event`, `_retire_state` (the retained-handle cases; the dropped case passes) |
| C3 | signals not read while a check is pending | `qr_queue_events_queued_ahead_of_a_reply_are_all_delivered`, `qr_queue_state_changes_queued_ahead_of_a_reply_are_all_delivered`, `qr_queue_an_event_flood_ends_the_subscription_without_stalling` |
| C4 | a post-decision period end withholds the item | `qr_queue_a_period_end_before_the_decision_withholds_and_one_after_retires_next` |
| C5 | the whole `f957d80` client library | `tui_watcher_reports_a_saturated_loss_without_the_old_changes`, `the_desktop_watcher_reports_a_saturated_loss_without_the_old_changes` (other TUI/desktop tests pass) |
| C6 | the guide's old step 4 (membership lines only) | `qr_recovery_the_documented_recovery_of_an_unknown_role_completes` |

**Correction found by C2.** The first run of C2 (at `e09f697`) failed nothing: the burst is fully
drained by `settle` before retirement, so nothing was left for a retained handle to hold back.
The retained-handle cases now add 40 more cycles and 200 more signals after retirement, with the
handle alive and never polled, before a second call (`128d44e`); C2 then fails them. The
incomplete-record (C6 passes `qr_recovery_deleting_only_the_membership_…`) and below-capacity
controls keep passing where they should.

### CI scheduling (QR-INTEGRATION-04)

- `.github/workflows/ci.yml` runs `dbus-run-session -- cargo test -p blueroute-client --test
  queue_saturation --locked -- --ignored` in the client owner-binding step.
- The TUI step's filter `-- --ignored tui_watcher` selects the new TUI test, and the desktop
  `watch_handover` step runs all of that binary's ignored tests, including the new outer test
  and its child scenario.
- The recovery unit tests and the zbus pin guard are ordinary tests in `cargo test --workspace`;
  the maintainer harness already runs in CI.
- `CLAUDE.md` lists the new suite.

## QR-VALIDATE — Exact software candidate (2026-11-14)

**Candidate.** `c317716c8878d7b679fb7f516261e961dc6719a6`, clean apart from the never-committed
`.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, Node 26.13.0, npm 11.20.0, Python 3.13.5,
dbus-daemon 1.16.2, Debian 13. Temporary files, package extraction and logs under the ignored
workspace `tmp/` (`TMPDIR` set there; `tmp/validate.sh` runs the whole sequence).

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check`; `cargo build`/`check --workspace --locked` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings`; same with `--features experimental-join` for the daemon | clean |
| `cargo test --workspace --locked` | 920 passed, 0 failed, 81 ignored (the ignored ones are the private-bus suites run below) |
| `cargo test -p blueroute-daemon --features experimental-join --locked` | 392 passed, 0 failed, 16 ignored |
| `cargo test -p blueroute-daemon --lib --locked removal` | 33 passed |
| Private brokers (`dbus-run-session`, each suite of `ci.yml`) | `dbus_service` 1, `authorization` 1, `dbus_client` 1, `cli_exit_contract` 1, `bridge_end_to_end` 1, `owner_race` 6, `event_owner_race` 6, `ownership_period` 12, `queue_saturation` 14, `owner_watch` 3, `watcher` 8, `state_events` 8, `publisher_progress` 6, `watch_integration` 2, `watch_handover` 6, `tui_watcher` 4: 80 passed, 0 failed |
| `python3 -m unittest discover -s scripts -p 'test_p6_006_*.py'` | 7 passed |
| Frontend (`apps/desktop`): `npm ci`, `format:check`, `lint`, `typecheck`, `test`, `build` | all rc 0; 4 files, 43 tests passed |
| `bash packaging/debian/build-package.sh` | both packages built |
| `test-maintainer-lifecycle.sh`, source scripts and the fresh package's extracted control scripts (`BLUEROUTE_MAINTAINER_SCRIPT_DIR`) | passed both (install, upgrade, remove, purge, abort, cleanup failure, unresolved role, manual recovery) |
| `test-package-upgrade.sh <fresh core .deb> target/release/examples/state_check` | passed: 0.0.1 → 0.1.0 kept identity, migrated v2 membership to v3 roles, restarted the service; failed cleanup aborted removal; removal retained state; purge deleted it. The first attempt failed only because `/usr/sbin` was not on `PATH` (dpkg needs `start-stop-daemon`); rerun with it added |
| Join closed by default | `default_builds_cannot_open_the_join_gate`, `the_production_runtime_is_closed_by_default_and_never_reaches_bluetooth` pass |
| Tauri permissions | `apps/desktop/src-tauri` and `apps/desktop/ui` unchanged since `f957d80` |
| Leftover test files | none in `tmp/` or `/tmp` after the full run |

**Packages** (not bit-reproducible; these hashes identify these files):

- `blueroute_0.1.0_amd64.deb` `3c61cfddbbe27b802e7bdd7fb6b66ccc38cdb372d19567a565d7d0b67b79dfef`:
  Depends `dbus, bluez, network-manager, polkitd, systemd`; files `usr/bin/blueroute`,
  `usr/bin/blueroute-tui`, `usr/libexec/blueroute/blueroute-daemon`,
  `usr/lib/systemd/system/blueroute.service`, the D-Bus policy and the polkit actions.
- `blueroute-desktop_0.1.0_amd64.deb`
  `e4ed17c91c8c9cc1b5ffbbfa2c74649e0863486e3653ca2f26529f7bcbc0cdd2`: Depends `blueroute (= 0.1.0)`
  plus the GTK/WebKit libraries; files `usr/bin/blueroute-desktop`, the desktop entry and icon.

**Preserved invariants (QR-INTEGRATION-05).** The same run covers publisher progress and
shutdown (`publisher_progress`), startup and graph/state publication (`state_events`), normal
owner handover (`owner_race`, `event_owner_race`, `watch_handover`), unknown-role proof retention
and stale-record cleanup (removal tests), UI effect generation (frontend tests), authorization
(`authorization`), pins/revocation, rollback and no-replay (workspace and experimental-join
suites), and closed join and hidden sharing (join gate tests; the frontend tests). No historical
count is treated as a fixed expectation; these are this candidate's results.

**Exact-revision CI.** [Run 43652001563](https://github.com/ekkus93/blueroute/actions/runs/43652001563) on `c317716`: success (Rust and Desktop frontend jobs), read with `gh run view`. Its log shows `queue_saturation` 14 passed, `saturated_loss_scenario`, `tui_watcher_reports_a_saturated_loss_without_the_old_changes`, the recovery tests and the zbus pin guard. Earlier runs are attributed to their own revisions: `e83e3a0` (QR-BASE) 43649272495, `ffce5bd` (QR-QUEUE) 43650421708, `0b58cef` (QR-RECOVERY) 43651245570, all success; `e09f697` and `128d44e` were pushed together with `c317716` and have no run of their own.

## QR-QUALIFY — Environment gates (2026-11-14)

- **QR-QUALIFY-01 (procedure).** `docs/CR-QUALIFY-PROCEDURE.md` A3 "Unknown active role,
  recovery" deletes both records, says why, and compares the foreign count (valid there because
  `H` stays known). The Part A kit (`tmp/qualify/part-a.tar.gz`, ignored, not versioned) was
  rebuilt at `c317716`: core package `3c61cfdd…` (the package validated above), fixture
  `a6ae47ad763372d500d599eca6bf0d2b9d7eeace39f92038ccee020b52a1f318`, upgrade package
  `03baad7eb3c83144af47914015d3328cb8ef01673b387366616fb4e4c552ffe3`; its runbook matches the
  corrected recovery. Procedure readiness is not execution.
- **QR-QUALIFY-02 (execution): blocked.** It needs a disposable Debian 13 VM with sudo (Part A)
  and the designated two-machine Bluetooth rig (Part B); neither is available to this work.
- **QR-QUALIFY-03 (deferral).** Recorded here. QR-QUALIFY, LC-QUALIFY-02/03, FU-QUALIFY-01..03 and
  CR-QUALIFY-01..08 stay open. No installed-system or physical claim is made.

# Code Review Follow-up Evidence

Durable evidence for [CODE_REVIEW_FOLLOWUP_TODO.md](CODE_REVIEW_FOLLOWUP_TODO.md); requirements in
[CODE_REVIEW_FOLLOWUP_SPEC.md](CODE_REVIEW_FOLLOWUP_SPEC.md). Entries are appended as tasks are
accepted; earlier entries are not rewritten. CI is checked with the GitHub CLI (`gh run`), per the
maintainer decision recorded in the TODO.

## FU-BASE — baseline and failing expectations (2026-11-13)

**FU-BASE-01.** HEAD at the start was `df02d09` = review baseline `6d200d0` plus the two follow-up
documents (no source difference). Untracked only: `.serena/`, `AGENTS.md` (never committed).
Toolchains: rustc 1.95.0, Node 26.13.0, npm 11.20.0. Guidance read: `CLAUDE.md`, `AGENTS.md`, the
follow-up specification, the first remediation specification and backlog.

**FU-BASE-02/03.** Each reproduced finding has a repository test stating the corrected contract,
committed `#[ignore]` with an `FU-xx regression` reason. Bus tests are in their own files, so CI's
`--ignored` runs of the existing suites stay green; each fix task enables its tests and adds them
to CI.

| Finding | Test | Command | Observed failure at the baseline |
| --- | --- | --- | --- |
| REV-01 | `apps/daemon/tests/publisher_progress.rs` `fu_01_a_pending_network_read_does_not_hold_back_a_health_change`: real `DaemonService` + production publisher on a private bus; the network list is held pending past one network recheck, then `update_runtime_observation` publishes `Degraded` | `dbus-run-session -- cargo test -p blueroute-daemon --test publisher_progress --locked -- --ignored` | "no StateChanged within 1s of a published health change while the network read was pending" |
| REV-02 (a) | `crates/blueroute-client/tests/event_owner_race.rs` `fu_02_an_event_whose_owner_hands_over_during_its_version_check_is_not_delivered`: A emits, then hands the name to compatible B inside that event's own Version call (call 3), staying connected | `dbus-run-session -- cargo test -p blueroute-client --test event_owner_race --locked -- --ignored` | "A's obsolete event was not retired: Some(Ok(Event(HealthChanged(Degraded))))" |
| REV-02 (b) | same file, `fu_02_owner_loss_is_reported_while_the_events_version_reply_is_held`: as (a), A then holds its reply until released | same | "owner loss not reported within 2 s of the handover: None" |
| REV-03 | `crates/blueroute-core/src/topology.rs` `fu_03_a_segment_cloned_from_another_graph_brings_no_phantom_members`: a populated segment cloned into a graph (a) without the client and (b) with the client but no link | `cargo test -p blueroute-core --lib --locked fu_03 -- --ignored` | "member NodeId([2, …]) is not a node" |
| REV-04 | `apps/daemon/src/removal.rs` `fu_04_unresolved_client_teardown_fails_and_keeps_its_proof_for_the_retry`: recorded client with owned profile/address and live owned PANU, plus a host-only network; cleanup without Bluetooth, then with it, then again | `cargo test -p blueroute-daemon --lib --locked fu_04 -- --ignored` | "unobserved client teardown reported success: RemovalReport { networks: 2, failed_networks: 0, remaining: 0, unresolved: 0, bluetooth: false }" |

The existing removal test `a_proven_owned_panu_link_is_disconnected_first_and_only_with_bluetooth`
still accepts success without Bluetooth; FU-04-04 corrects it. GAP-01 (startup baseline) and
GAP-02 (qualification fixture) are not reproduced defects: GAP-01 gets barrier tests in FU-05,
GAP-02 is a procedure correction in FU-06.

**FU-BASE-04.** Fixtures: a held network read and a held Version reply both poll a flag that a
drop guard releases, so a failing assertion cannot hang the test or the publisher's shutdown; each
waits with an explicit bound; ownership is explicit NetworkIds (known, foreign, other network);
bus tests serialize on a mutex because they share the well-known name.

**FU-BASE-05.** Isolation: bus tests use the private session bus of `dbus-run-session`; state in
temporary directories; removal tests use the fake backend; the isolated dpkg test replaces the
installed maintenance helper with a mock before any `prerm remove` runs (install and upgrade never
call it), and its `rm` guard rejects paths outside the isolated root. `DPKG_ROOT` relocates paths
only and does not isolate D-Bus; that limitation is FU-06-04's to document in the procedure.

## FU-01 — independent publication and bounded shutdown (2026-11-13)

**Fix:** `19a58a0` (`apps/daemon/src/state_publisher.rs`; SPEC §11).

- **FU-01-01 design** (module documentation): a sampler thread owns the only slow read
  (`observable_networks`: membership store, BlueZ discovery). It samples once at start, every
  `NETWORK_RECHECK` (1 s) and on `committed()`, one sample at a time, each bounded by
  `SAMPLE_DEADLINE` (10 s; a timeout is a failed read, logged payload-free as
  `network_sample`/`timed_out`). It stores the latest result in a slot and unparks the emitter.
  The emitter never awaits a sample: it compares the in-memory snapshot and the slot with what it
  last announced; the first sample is the network baseline. Sampler deadline + stop polling are the
  only termination paths for a blocked read; no thread is detached.
- **FU-01-02:** the initial and periodic network reads moved off the emitter.
- **FU-01-03:** the forced flag is re-set when an emission fails (retried every 250 ms); a stored
  sample only advances the announced baseline after a successful emission; samples are taken
  sequentially, so no older result overwrites a newer one; a failed or timed-out read is `None`,
  distinct from an empty list.
- **FU-01-04:** exactly two threads; `Drop` sets the stop flag, which cancels a pending sample or
  emission within `STOP_POLL` (50 ms), then joins both.

**Regressions** (`apps/daemon/tests/publisher_progress.rs`, private bus, in CI; 5 consecutive
runs green):

| Test | Covers | Result |
| --- | --- | --- |
| `fu_01_a_pending_network_read_does_not_hold_back_a_health_change` | PUB-1/4 (REV-01) | pass, delivered ≈0.6 ms after publication; **failed at 6d200d0** (FU-BASE) |
| `fu_01_a_read_pending_from_startup_does_not_block_announcements` | initial sample never finishes | pass |
| `fu_01_shutdown_finishes_while_a_sample_is_pending` | PUB-3: shutdown ≈0.2 ms with the read held (bound 2 s) | pass; on 6d200d0 `Drop` joins the blocked thread, i.e. waits for the read |
| `fu_01_a_released_read_brings_the_final_network_state` | PUB-2/4 final state after release | pass |
| `fu_01_a_failing_sample_is_announced_once_and_its_recovery_too` | failure is a change once, no loop, recovery announced, other changes still prompt | pass |

Unchanged samples, bursts and emitter failure/retry remain covered by `state_events` (8 pass).
The startup test waits for the emitter's baseline before publishing, because a change racing
that baseline is GAP-01 (FU-05), observed as an intermittent failure without the wait.

## FU-02 — ownership monitored through validation (2026-11-13)

**Fix:** `d2b94c1` (`crates/blueroute-client/src/lib.rs`, `EventSubscription::next_change`; SPEC
client bullet).

**FU-01 decision ordering:** (1) wait for a signal or an owner retirement; (2) race the bound
owner's Version check against the owner-change stream; (3) after a successful check, require
`GetNameOwner` to still return the bound owner — the delivery decision point; (4) decode and
deliver. A retirement at (1), (2) or (3) sets `retired` and returns `OwnerLost`; every later call
returns `OwnerLost` (terminal) and the pending check, with any late reply, is dropped. Queued
notices that only record the bound owner taking the name are skipped; any other notice retires.
Each notice is read and decided without an intervening await, so cancelling a `next_change`
cannot consume a retirement silently. Commands still go only to the unique owner validated for
them (CR-F8), with no retry or replay.

**Regressions** (`crates/blueroute-client/tests/event_owner_race.rs`, private bus, in CI; three
consecutive runs green):

| Test | Covers | Result |
| --- | --- | --- |
| `fu_02_an_event_whose_owner_hands_over_during_its_version_check_is_not_delivered` | OWNER-2 (REV-02 a) | pass; **failed at 6d200d0** |
| `fu_02_owner_loss_is_reported_while_the_events_version_reply_is_held` | OWNER-1/3 (REV-02 b), late reply and later events not delivered | pass; **failed at 6d200d0** |
| `fu_02_an_incompatible_successor_after_an_event_time_handover_is_never_used` | OWNER-3: B (API 99.0) refused, zero Requests reach it | pass |
| `fu_02_an_owner_leaving_with_no_successor_during_validation_is_reported` | no replacement, reply held | pass |
| `fu_02_an_interrupted_wait_loses_nothing_and_a_fresh_subscription_follows_b` | cancellation, terminal retirement, fresh subscription to B | pass |
| `fu_02_the_shared_watcher_moves_to_b_after_an_event_time_handover` | FU-02-07: A's obsolete event is not a `Changed`; `Connected → Disconnected → Connected` to B, then B's change | pass |

The desktop and TUI run the same `watch` (their adapters are covered by `watch_integration` and
`tui_watcher`, both green). Existing `owner_race` (6), `owner_watch` (3), `watcher` (6) and
`dbus_client` (1) pass unchanged, so command-owner safety and idle owner loss are intact.

## FU-03 — safe segment cloning/import (2026-11-13)

**Fix:** `1f2d48d` (`crates/blueroute-core/src/topology.rs`; SPEC
`blueroute-core` invariants).

**FU-03-01 policy: normalize.** `add_segment` of a new SegmentId rebuilds the stored member set
from the destination graph: the segment's current NAP, its explicit members (validated as nodes
before any mutation), and the PANU of every link in this graph that belongs to the segment with
the same NAP. Members the input carried from another graph's links are discarded. A changed public
`nap` on a clone is simply the NAP used. Reinsertion of an existing SegmentId keeps the stored
segment (and merges new explicit members), a different NAP is refused, and link semantics are as
before. No caller changes were needed: all graph callers are tests.

**Regressions** (`cargo test -p blueroute-core --lib fu_03`, 4 tests, all pass; 148 core tests
pass):

| Test | Covers |
| --- | --- |
| `fu_03_a_segment_cloned_from_another_graph_brings_no_phantom_members` | REV-03: clone with the client missing and with it present but unlinked; whole-graph consistency; **failed at 6d200d0** |
| `fu_03_a_cloned_segment_with_a_changed_nap_is_rebuilt_or_refused_atomically` | changed NAP rebuilt in a fresh graph, refused (graph equal) where segment 1 exists; unknown NAP and unknown explicit member refused atomically |
| `fu_03_explicit_members_and_same_graph_reinsertion_still_work` | identical reinsertion keeps link members; explicit non-link member kept |
| `fu_03_phantom_membership_cannot_reach_routing` | downstream: no path or neighbor from phantom membership; the real link makes the node a member and reachable |

Property-only link updates, multi-link removal and the forwarding consumer remain covered by the
CR-F3 tests, which pass unchanged.

## FU-04 — unresolved client cleanup fails and stays retryable (2026-11-13)

**Fix:** `718983c` (`apps/daemon/src/removal.rs`, `client_leave.rs`, `main.rs`; design
[CR-F5-REMOVAL-CLEANUP.md](CR-F5-REMOVAL-CLEANUP.md) revised; SPEC §31; `ADMINISTRATOR.md`).

- **FU-04-01 outcomes:** for a joined-client network (persisted role `client`, or named by the
  client record), *observed absent* = Bluetooth up, recorded host device, and no live PANU link to
  it (or the proven-owned one disconnected and then observed gone). Everything else is
  *unresolved*: counted in `RemovalReport::unresolved_links` / log field
  `unresolved_client_links`, and `succeeded()` is false.
- **FU-04-02 proof:** maintainer decision — keep the existing NetworkManager ownership proof (owned
  profile and address carrying the NetworkId); no new record, no storage or schema change. A host
  hint alone never authorizes a disconnect: the live link must be to that device *and* on an
  interface carrying this network's ownership.
- **FU-04-03:** an unresolved client network is left untouched; other networks are cleaned.
  `leave_observed` re-observes after `disconnect_panu` and fails (proof kept) if the link is still
  there — this also hardens user-initiated client leave.
- **FU-04-07:** `cleanup_outcome()` maps the report to exit status and log line (unresolved → 1);
  `prerm` propagates it (lifecycle test: failure fails `prerm`, `abort-remove` restores the
  service; the isolated dpkg test covers real remove failure/retry/purge). Upgrades are untouched.
- **FU-04-08:** instance lock, removal marker, 60 s bound and payload-free logging unchanged; no
  shell networking added.

**Regressions** (`cargo test -p blueroute-daemon --lib removal`, 17 tests, all pass):

| Test | Covers |
| --- | --- |
| `fu_04_unresolved_client_teardown_fails_and_keeps_its_proof_for_the_retry` | REV-04: without Bluetooth fails, keeps proof and link, host-only network still cleaned; with Bluetooth disconnects and succeeds; repeat succeeds. **Failed at 6d200d0** |
| `a_proven_owned_panu_link_is_disconnected_with_bluetooth_and_unresolved_without` | the corrected former fixture (FU-04-04) |
| `fu_04_an_already_absent_link_with_bluetooth_up_is_proven_gone` | absent link; also the state after an interruption between disconnect and proof removal |
| `fu_04_a_disconnect_that_does_not_take_keeps_the_proof` | interrupted/failed disconnect: failure, nothing removed, retry succeeds |
| `fu_04_a_link_to_another_device_on_a_reused_interface_name_is_left_alone` | reused interface name |
| `fu_04_without_proof_a_live_link_is_never_disconnected` | vanished and conflicting proof: link untouched |
| `fu_04_a_client_network_known_only_by_role_is_unresolved_without_a_host_device` | client record missing: unresolved even with Bluetooth |
| `an_unresolved_client_link_makes_the_helper_exit_nonzero` | the helper's exit status and log line |
| existing: foreign preserved, partial failure/retry, repeat, unreachable/blind backend, timeout, running daemon, marker, bad registry, no known network | unchanged, pass |

Client leave tests (10) pass with the new re-observation; lifecycle test and `maintenance_mode`
(4) pass. Installed-system and physical qualification stay with FU-QUALIFY / CR-QUALIFY.

## FU-05 — startup snapshot/publication handoff (2026-11-13)

**Fix:** `c104bea` (`apps/daemon/src/state_publisher.rs`; SPEC §11; dated correction appended to
the earlier remediation evidence, FU-05-04).

**FU-05-01 ordering:** the service may be available (name owned, snapshots readable) before the
publisher's baselines exist. Instead of relying on startup being faster than any client, the
publisher announces once as soon as the emitter runs (the forced flag is set at spawn) and once
when the first network sample lands, regardless of differences. Clients subscribe and then read
(the shared watcher reads on `Connected`), so any snapshot read before a baseline is followed by an
invalidation. A change after the baseline is detected normally. No exactly-once delivery is
claimed; at most two extra startup signals occur.

**Regressions** (private bus, in CI):

| Test | Covers | With the fix | Handoff removed (negative control) |
| --- | --- | --- | --- |
| `publisher_progress::fu_05_a_change_before_the_first_baseline_still_reaches_a_client` | START-1/3: subscribe → read (healthy) → change → start publisher; repeated with the network read held | pass | **fail** ("absorbed into the startup baseline") |
| `watcher::fu_05_a_replacement_daemons_pre_baseline_change_reaches_the_watcher` | START-2/3: shared watcher reconnects to compatible B and rereads; B changes before its publisher starts | pass | **fail** |

Test adjustments, not weakenings: observers and fixtures now consume the startup announcements
before measuring, so a test only counts signals it caused; the failing-read test first lets the
sampler's single "store failing" announcement (a legitimate change, FU-01) pass, then requires zero
signals from five failing client reads. `publisher_progress` (6), `state_events` (8) and `watcher`
(7) passed three consecutive runs.

## FU-06 — honest, adapter-free qualification prerequisites (2026-11-13)

**Change:** `f3d875a` ([CR-QUALIFY-PROCEDURE.md](CR-QUALIFY-PROCEDURE.md);
`apps/daemon/examples/qualify_fixture.rs`).

| Subtask | Result |
| --- | --- |
| 01 fixture | `qualify_fixture host|foreign|client` creates owned state through `NetworkManagerBackend::ensure_bridge/ensure_address` and the production membership and client-record stores, so ownership metadata (`org.blueroute.owner`, `kind`, `schema=1`), profile names and permissions are exactly the daemon's. The procedure lists the expected resources, verification and teardown (snapshot revert). |
| 02 VM dependencies | Disposable Debian 13 VM, no adapter; foreign bridge/address/route and optional `ip_forward=1` comparisons; independent `nmcli`/`ip`/`sysctl`/`dpkg` snapshots for each case. |
| 03 layering | A seeded client with no Bluetooth must fail (`unresolved_client_links`, proof kept) and is explicitly not client teardown; real NAP/PANU teardown, including Bluetooth down-then-up, is Part B only. |
| 04 isolation audit | Private-bus tests and fakes never reach host services; `maintenance_mode` cases exit before any bus even as root; the isolated dpkg harness mocks the helper because `DPKG_ROOT` relocates files, not D-Bus — now stated in the procedure. The fixture is outside every production path and is not built by `build-package.sh` (which builds only the daemon, CLI and TUI binaries). |
| 05 checklist | Expected log fields (`unresolved_client_links`, `bluetooth_available`), statuses, equipment and evidence fields updated; status reads "procedure ready, NOT EXECUTED". |

Procedure readiness is not qualification: FU-QUALIFY / CR-QUALIFY stay open.

## FU-INTEGRATION — production boundaries and regression sensitivity (2026-11-13)

**Commit:** `41417a9` (plus the per-finding tests above).

| Subtask | Evidence |
| --- | --- |
| 01 | `watcher::fu_integration_the_watcher_hears_real_changes_while_network_sampling_is_stalled`: real `DaemonService` + production publisher + production `watch`, network read stalled from a cold start; a change made before the publisher exists and a later change each reach the watcher within 1 s. Also `fu_05_a_replacement_daemons_pre_baseline_change_reaches_the_watcher`. |
| 02 | Desktop `watch_handover` (unchanged `watch_daemon`, system-bus address pointed at the private bus) and TUI `tui_watcher_follows_an_event_time_handover_to_the_successor` (real watcher thread via `spawn_with`): `Connected → Disconnected → Connected` (successor) → `Changed`, never A's obsolete change. Incompatible-owner refusal with zero Requests and no replay: `event_owner_race`. |
| 03 | Imported graph through a consumer: `fu_03_phantom_membership_cannot_reach_routing`. Unresolved cleanup through the packaged path: `cleanup_outcome` maps an unresolved link to exit 1 (tested), and that status fails `prerm` in the lifecycle test and real `dpkg --remove` (isolated harness, mocked helper exit 1, then retry). The real helper against real services is FU-QUALIFY. |
| 04 | CI runs every new bus suite: `publisher_progress`, `event_owner_race`, `watch_handover`; new tests in `watcher`, `state_events` and the TUI `tui_watcher` filter run in existing steps. Exact invocations in `CLAUDE.md`. Nothing required is left ignored outside a CI step; the only environment-dependent pieces are the private-bus suites (run under `dbus-run-session`). |
| 05 | Negative controls below. |

**Negative controls** (each defect restored in the working tree, tests run, file restored to HEAD
and verified with `git diff`):

| Restored | How | Failing tests |
| --- | --- | --- |
| REV-01 | blocking `observable_networks()` read put back inside the emitter loop | `fu_01_a_pending_network_read…` (publisher_progress); `fu_integration_…stalled` (watcher) |
| REV-02 | Version check no longer raced against owner changes; post-check owner verification disabled | desktop `watch_handover`; TUI handover test; `fu_02_the_shared_watcher…`; 5 of 6 `event_owner_race` |
| REV-03 | new segment stored as given | 2 of 4 `fu_03` tests (clone and changed-NAP) |
| REV-04 | unobservable client link skipped, proof removed, success | 3 of 17 removal tests (REV-04 regression, corrected fixture, role-only client) |
| GAP-01 | startup announcements removed (FU-05 entry) | `fu_05_a_change_before_the_first_baseline…`; `fu_05_a_replacement_daemons…` |

The REV-01..04 baseline failures at `6d200d0` are in the FU-BASE entry.


## FU-CLOSEOUT-01 — independent review of 6d200d0..HEAD (2026-11-13)

An independent review read the whole follow-up diff against the follow-up specification and
reran the new private-bus suites. It confirmed the publisher (no lost dirty state, bounded
shutdown, startup announcements), the client (no lost or stale delivery, no deadlock), the graph
normalization and the fixture's isolation, and found three gaps, fixed in `4d19a2a`:

| Gap | Fix | Evidence |
| --- | --- | --- |
| REMOVE-1: with `bluetoothd` running but no object for the recorded host device, "no link observed" was treated as proof of absence | `BluezBackend::panu_device_present`; a missing device is unresolved | `fu_04_a_missing_host_device_is_inconclusive_even_with_bluetooth_running` (fails without the check) |
| A client record left behind by a leave made a network a "client" and blocked removal without Bluetooth | only the persisted `client` role decides; the record supplies the hint | `fu_04_a_stale_client_record_does_not_block_removal`; REV-04 fixtures now set the role |
| OWNER-2: a transient `GetNameOwner` failure permanently retired a healthy subscription | only `NameHasNoOwner` retires | existing `event_owner_race` no-successor case exercises `NameHasNoOwner`; a bus `NoReply` cannot be provoked deterministically (limitation) |

Accepted as is: `leave_observed`'s re-observation can fail a user leave on a transient BlueZ
error after a successful disconnect; membership stays and a retry observes the link absent.

## FU-VALIDATE — exact software candidate (2026-11-13)

**Candidate:** `4d19a2a5bac375fda3e84a7a1bdb83072e8c358d`; exact-head [CI 43536690676](https://github.com/ekkus93/blueroute/actions/runs/43536690676) passed (Rust and
Desktop frontend jobs, all bus-suite steps). Later commits only add evidence documents.
Toolchains: rustc 1.95.0, Node 26.13.0, npm 11.20.0. Earlier results belong to their SHAs
(`8b5eca1`: CI 43535659753; `c104bea`: 43534489978; `eb1975b`: 43533417640).

| Subtask | Commands | Result at the candidate |
| --- | --- | --- |
| 01 | `cargo build/check --workspace --locked`; `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo test --workspace --locked` | clean; 901 passed, 0 failed, 49 ignored (the private-bus suites, run in 02) |
| 02 | `cargo test -p blueroute-daemon --features experimental-join --locked`; every private-bus suite under `dbus-run-session`; `python3 -m unittest discover -s scripts -p 'test_p6_006_*.py'` | 378 passed (default build: `default_builds_cannot_open_the_join_gate` passes); dbus_service 1, authorization 1, dbus_client 1, cli_exit_contract 1, bridge_end_to_end 1, owner_race 6, event_owner_race 6, owner_watch 3, watcher 8, state_events 8, publisher_progress 6, watch_integration 2, watch_handover 2, tui_watcher 2; Python 7 OK |
| 03 | `npm ci`, `format:check`, `lint`, `typecheck`, `test`, `build` | all pass; 43 tests incl. the 8 StrictMode/effect-generation tests |
| 04 | `build-package.sh`; packaged-script lifecycle test; `test-package-upgrade.sh` | `blueroute_0.1.0_amd64.deb` sha256 `50cda9057e1f7c4e8ef06c639dca23ddfa884084544e9d1a87bef198119e2dbf`; `blueroute-desktop_0.1.0_amd64.deb` sha256 `dc30172ffb9375d4bd11f00d06dfd489ff09e4d8e3687ec3b5c1f2917132e750`; Depends unchanged; frontend assets embedded (build check); `qualify_fixture` not in the package; lifecycle and isolated upgrade/migration/remove-failure/retry/purge pass. Local builds are not bit-reproducible. |
| 05 | invariant regressions inside the workspace suite (authorization, owner binding/no replay, pins, exporter, revocation, rollback, foreign preservation, identity, schema migration, disabled sharing) | pass |
| 06 | this record | candidate, commands, counts, hashes, CI run |

No installed-system or physical claim follows from these results.

### Requirement map (FU-CLOSEOUT-04)

| Requirement | Task | Implementation | Evidence |
| --- | --- | --- | --- |
| PUB-1 separate progress | FU-01 | sampler thread; emitter never awaits a sample | `fu_01_a_pending_network_read…`, `fu_integration_…stalled` |
| PUB-2 preserve dirty state | FU-01 | forced flag re-set on emission failure; sequential samples; failed read ≠ empty list | `fu_01_a_released_read…`, `fu_01_a_failing_sample…`, `state_events` emission-failure and burst tests |
| PUB-3 bounded lifecycle | FU-01 | `STOP_POLL` cancellation, `SAMPLE_DEADLINE`, join both threads | `fu_01_shutdown_finishes_while_a_sample_is_pending` |
| PUB-4 observable acceptance | FU-01 | — | `publisher_progress` (6), `fu_integration_…stalled` |
| OWNER-1 one lifecycle | FU-02 | Version check raced against owner changes | `fu_02_owner_loss_is_reported_while_the_events_version_reply_is_held` |
| OWNER-2 validate delivery | FU-02 | `GetNameOwner` decision point after the check | `fu_02_an_event_whose_owner_hands_over…` |
| OWNER-3 terminal retirement | FU-02 | `retired` flag | `fu_02_an_interrupted_wait…`, held-reply test (late reply), incompatible-successor test |
| OWNER-4 scenarios | FU-02, FU-INTEGRATION | — | `event_owner_race` (6), desktop `watch_handover`, TUI handover test |
| SEG-1 insertion contract | FU-03 | normalization documented in `add_segment` and SPEC | `fu_03_a_segment_cloned…` |
| SEG-2 whole-graph invariants | FU-03 | members rebuilt from destination evidence | `assert_consistent` in all `fu_03` tests |
| SEG-3 atomicity and identity | FU-03 | validation before mutation; changed NAP followed or refused | `fu_03_a_cloned_segment_with_a_changed_nap…`, `fu_03_explicit_members…` |
| SEG-4 scenarios | FU-03 | — | 4 `fu_03` tests incl. routing consumer |
| REMOVE-1 explicit observation | FU-04 | `NetworkScope.client`; unresolved without Bluetooth or hint | `fu_04_unresolved_client_teardown…`, role-only client test |
| REMOVE-2 proof lifetime | FU-04 | NM proof kept until the link is observed gone (`leave_observed` re-observes) | `fu_04_a_disconnect_that_does_not_take…`, without-proof test, reused-name test |
| REMOVE-3 partial progress/retries | FU-04 | other networks cleaned; retry reobserves | REV-04 regression (host-only cleaned, retry, repeat), absent-link test |
| REMOVE-4 outcome and lifecycle | FU-04 | `unresolved_links` in `succeeded()`; `cleanup_outcome` exit 1 | `an_unresolved_client_link_makes_the_helper_exit_nonzero`, lifecycle and isolated dpkg tests |
| REMOVE-5 scenarios | FU-04 | — | 17 removal tests |
| START-1 initialization ordering | FU-05 | announce at start and at first sample | `fu_05_a_change_before_the_first_baseline…` (sampling free and held) |
| START-2 client handoff | FU-05 | watcher subscribes then rereads on `Connected` | `fu_05_a_replacement_daemons_pre_baseline_change…` |
| START-3 regression evidence | FU-05 | — | both FU-05 tests fail with the handoff removed |
| QUAL-1 adapter-free Part A | FU-06 | `qualify_fixture` | procedure Part A |
| QUAL-2 evidence layers | FU-06 | layers table; seeded client must fail | procedure |
| QUAL-3 isolation | FU-06 | `DPKG_ROOT` statement; fixture outside production | procedure; packaged contents checked |
| QUAL-4 deferred acceptance | FU-06, FU-QUALIFY | — | FU-QUALIFY open |
| ACCEPT-1 useful regressions | FU-BASE, FU-INTEGRATION | — | baseline failures; negative controls |
| ACCEPT-2 exact candidate | FU-VALIDATE | — | FU-VALIDATE entry |
| ACCEPT-3 reconcile status | FU-CLOSEOUT | dated corrections | first backlog, first evidence, `TODO.md` |
| ACCEPT-4 separate milestones | FU-CLOSEOUT | — | SOFTWARE COMPLETE / QUALIFICATION DEFERRED |


## FU-CLOSEOUT — software milestone (2026-11-13)

- **01:** each fix reviewed against its requirements as it was accepted, plus the independent
  review above; its three gaps are fixed at `4d19a2a`.
- **02:** dated corrections appended to the first backlog (CR-F8, CR-F2, CR-F1, CR-F3, CR-F5,
  CR-INTEGRATION, CR-CLOSEOUT; its status line now points here) and to the first evidence record;
  historical entries kept.
- **03:** SPEC (§11 publication and startup, client bullet, `blueroute-core` invariants, §31
  removal), the cleanup design, the qualification procedure and `ADMINISTRATOR.md` describe the
  current behavior; `TODO.md` points to this backlog; no P0–P19 checkbox changed.
- **04:** requirement map above; all links checked.
- **05:** **SOFTWARE COMPLETE / QUALIFICATION DEFERRED.** FU-QUALIFY (CR-QUALIFY-01..08) has not
  run; full remediation closure waits for it.

## Correction — lifecycle review findings (2026-11-14)

A review of `75c1a01` found two P2 defects that the entries above did not cover. The entries
are kept as recorded; their results hold for the cases they tested.

- **RFU-01 (FU-02, FU-INTEGRATION-02):** the FU-02 decision point compared `GetNameOwner`'s unique
  name with the bound owner. An A→B→A or A→unowned→A transfer therefore passed it, and the old
  period's item was delivered. Fixed by LC-OWNER (`0fdb68f`).
- **RFU-02 (FU-04, FU-INTEGRATION-03):** `removal_scope` derived host-only safety from
  `role != Some(Client)`. A `member` network with no role, and a client record with no
  membership, were cleaned as host-only. Fixed by LC-CLEANUP (`0106e3a`).

Evidence, negative controls and the corrected software state are in
[CODE_REVIEW_LIFECYCLE_EVIDENCE.md](CODE_REVIEW_LIFECYCLE_EVIDENCE.md).

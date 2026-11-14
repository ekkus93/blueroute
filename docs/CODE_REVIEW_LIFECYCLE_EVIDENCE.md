# Code Review Lifecycle Remediation Evidence

Versioned evidence for [CODE_REVIEW_LIFECYCLE_TODO.md](CODE_REVIEW_LIFECYCLE_TODO.md) and
[CODE_REVIEW_LIFECYCLE_SPEC.md](CODE_REVIEW_LIFECYCLE_SPEC.md). Each entry names the exact source
revision it was taken at. Software results here are not installed-system or physical evidence.

## LC-BASE — Findings preserved and corrected expectations established (2026-11-14)

**Environment.** Source `e167198` (lifecycle docs only on top of the reviewed baseline
`75c1a01746557bb13f525a55642b497e3bd5f81d`; no code difference). Clean tree apart from the
never-committed `.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, zbus 5.19.0 (async-io), Debian
13 kernel 6.12, private brokers via `dbus-run-session`. Guidance read: `CLAUDE.md`,
`docs/SPEC.md`, the follow-up and lifecycle specifications and backlogs.

### RFU-01 — a returning owner bypasses retirement (LC-BASE-02/03)

- **Trigger.** A subscription bound to owner A sees A emit an item. Inside that item's Version
  check A releases the name, B takes and releases it, and A takes it back, then A replies.
- **Affected path.** `crates/blueroute-client/src/lib.rs`, `EventSubscription::next_change`: the
  Version reply races the ownership watch with `future::or`, which polls the reply first, and the
  final check compares only `GetNameOwner`'s unique name, which is A again.
- **Earlier tasks.** FU-02-02/03 and FU-INTEGRATION-02 accepted A→B handover only; OWNER-1..4.
- **Corrected expectation.** `crates/blueroute-client/tests/ownership_period.rs`. The fake daemon
  is a raw message loop, so the test knows when its reply was sent; after replying it calls a
  barrier object on the client's own connection, which travels behind the reply, so once it is
  answered the reply and all four ownership notices are queued at the client. The consumer is
  polled only until A receives the Version call and then paused until that acknowledgement.
  No sleep establishes the order.
- **Baseline result** (`dbus-run-session -- cargo test -p blueroute-client --test
  ownership_period --locked -- --ignored`, at `e167198`): 1 passed, 2 failed.
  - `lc_owner_a_returning_owner_does_not_deliver_its_obsolete_event`: *"the old period's Event
    was not retired: Some(Ok(Event(HealthChanged(Degraded))))"*.
  - `lc_owner_a_returning_owner_does_not_deliver_its_obsolete_state_change`: *"the old period's
    State was not retired: Some(Ok(State { revision: 7 }))"*.
  - `lc_owner_a_signal_queued_before_a_release_and_reacquire_is_not_delivered` passed: there the
    Version call is issued only after the notices are queued, and the ownership branch of the
    race wins because its item is already ready. It is kept as a maintained regression for
    EPOCH-2, but it is not evidence of the defect.
- **Scope of the reproduction.** One private broker, fake daemon; Event and StateChanged
  deliveries share `next_change`. No physical or installed-system claim.

### RFU-02 — an unknown active role bypasses client cleanup (LC-BASE-02/04)

- **Trigger.** The production membership store holds `Member` with role `none` for a network,
  and the production client-record store holds a record for the same network. Cleanup runs
  without Bluetooth.
- **Affected path.** `apps/daemon/src/removal.rs`, `removal_scope`: `client` is
  `role == Some(Client)`, so the unknown role becomes `NetworkScope { client: false, .. }` and the
  network is cleaned as host-only.
- **Earlier tasks.** FU-04-01/02/03/06/07/09, FU-INTEGRATION-03; REMOVE-1..5.
- **Corrected expectation.** `lc_cleanup_an_unknown_active_role_with_a_client_record_stays_unresolved`
  in `apps/daemon/src/removal.rs` seeds both stores through their production `save`, runs the
  production `run_removal_cleanup` against the in-memory NetworkManager/BlueZ fake, and requires
  failure, the owned profile and address still present, no disconnect, and the membership and
  record unchanged.
- **Baseline result** (`cargo test -p blueroute-daemon --lib --locked lc_cleanup`, at
  `e167198`): failed with *"an unknown active role was cleaned as host-only: RemovalReport {
  networks: 1, failed_networks: 0, remaining: 0, unresolved: 0, unresolved_links: 0, bluetooth:
  false }"*. The test is unit-level and runs in every workspace test run, so it is committed with
  its fix (LC-CLEANUP) rather than ahead of it; this baseline failure is its record.

### Isolation (LC-BASE-05)

- The ownership tests use only the `dbus-run-session` session bus, which they serialize with a
  file-local lock; every connection is dropped at the end of its scenario.
- The cleanup tests use a per-test temporary state directory and the in-memory backend; no
  NetworkManager, BlueZ or system bus is reached.
- Package-level tests (`packaging/debian/test-maintainer-lifecycle.sh`) already use a temporary
  `DPKG_ROOT` with a mocked helper, so they cannot reach host buses. LC-INTEGRATION extends them.

## LC-OWNER — Subscriptions retire when their ownership period ends (2026-11-14)

**Source.** The LC-OWNER commit on top of `5845584` (`crates/blueroute-client/src/lib.rs`).

### Design (LC-OWNER-01..03)

- **Ownership period.** The subscription records `since`: the receive position (zbus
  `Message::recv_position`) of the `GetNameOwner` reply that named its owner during validation.
  A `NameOwnerChanged` notice received after `since` that leaves the name with anyone but that
  owner ends the period. An owner's own acquisition notice does not, and nor do notices
  received before `since`, which describe earlier history. An unreadable notice ends the period.
- **Delivery decision point.** After an item's Version check, the final `GetNameOwner` reply is
  the decision point. The item is delivered only if (a) the reply names the bound owner and
  (b) no period-ending notice was received before that reply. zbus's single socket reader hands
  each message to every stream before it reads the next one, so when the reply is in hand every
  earlier notice is already queued and can be settled without waiting (`ended_before`).
  Comparing names alone (the 75c1a01 behavior) cannot distinguish A from A-after-B.
- **Progress.** The period stays watched while the subscription waits for signals and while the
  Version reply is pending, so neither a held reply nor a paused signal source delays the loss.
- **Terminal and cancellation-safe.** Every detected end sets the persistent `retired` flag in
  the same poll that consumes the notice; a cancelled wait therefore cannot consume and forget
  it. A period-ending notice received *after* a valid decision does not withdraw that
  delivery; it retires the subscription for the next call. A returning owner is reachable only
  through a fresh subscription, which validates compatibility again.
- **Transient errors.** Only `NameHasNoOwner` from the final check retires (`shows_no_owner`).
  Timeouts, no-reply, limits and I/O errors are returned without ending a healthy subscription.
- **Unchanged.** API 1.7, no wire epoch field, unique-owner Version/Request destinations,
  major/minor compatibility rules and no mutation replay. No polling: the drain is a
  non-blocking read of already-queued notices, and no task or buffer is added.

### Regressions (LC-OWNER-04..07)

`crates/blueroute-client/tests/ownership_period.rs` (private broker, raw fake daemon, barrier
acknowledgements). All 9 pass with the fix:

| Test | Scenario |
| --- | --- |
| `lc_owner_a_returning_owner_does_not_deliver_its_obsolete_event` / `…_state_change` | A→B→A inside the item's Version check; consumer paused until the reply and notices are queued; OwnerLost first, every later wait retired, fresh subscription hears A's new period |
| `lc_owner_a_signal_queued_before_a_release_and_reacquire_is_not_delivered` | A→unowned→A with old Event and StateChanged queued before selection |
| `lc_owner_a_handover_and_return_during_final_verification_is_not_delivered` / `lc_owner_a_release_and_reacquire_during_final_verification_is_not_delivered` | The period ends after A replied, while the final check is pending |
| `lc_owner_a_held_reply_does_not_delay_the_end_of_the_period` | A→B→A then a held reply; actively polled loss within 2 s; late reply changes nothing |
| `lc_owner_cancelling_during_validation_does_not_erase_the_end_of_the_period` | The wait is dropped mid-validation; the next wait reports OwnerLost |
| `lc_owner_a_returning_owner_is_validated_again_before_a_fresh_subscription` | A returns incompatible: the fresh subscription is refused and no Request reaches A |
| `lc_owner_a_continuous_period_keeps_delivering` | Healthy A delivers Event and StateChanged items in order; Requests still reach A |

Unit test `lc_owner_only_a_missing_owner_retires_on_a_failed_final_check` covers the
transient-error classification, since a broker-level GetNameOwner fault cannot be injected.

Preserved suites, all passing with the fix (private brokers): `event_owner_race` 6,
`owner_race` 6, `owner_watch` 3, `watcher` 8, `dbus_client` 1; desktop `watch_integration` 2
and `watch_handover` 2; TUI `tui_watcher` 2; client unit tests 4.

### Negative controls (isolated worktree, shared target directory)

| Control | Result |
| --- | --- |
| Fix with the ordered decision drain disabled | 5 of 9 fail: both returning-owner cases, both final-verification cases, revalidation |
| Baseline `75c1a01` subscription code | The same 5 fail |

The 4 that pass under both controls are held-reply progress, cancellation, the queued-signal
A→unowned→A case and the healthy period. The baseline already passes them, because its
ownership watch wins when the notice is ready before the Version reply. They guard against
regressions rather than detect RFU-01.

### Limitations

- The "notice received after a valid decision" branch is reasoned, not exercised. Its window is
  between two consecutive messages at the client, and a broker test cannot place a notice
  there deterministically.
- The queue-ordering argument relies on zbus 5.19's single socket reader, which is documented
  for `MessageStream::poll_next_before`. A zbus upgrade must keep that property.

## LC-CLEANUP — Ambiguous active membership stays unresolved (2026-11-14)

**Source.** The LC-CLEANUP commit on top of `0fdb68f`: `apps/daemon/src/removal.rs`,
`apps/daemon/src/lib.rs` and `packaging/debian/test-maintainer-lifecycle.sh`. Docs updated:
`docs/SPEC.md`, `docs/CR-F5-REMOVAL-CLEANUP.md` ("Classification") and `docs/ADMINISTRATOR.md`
("Removal with an unresolved role").

### Design (LC-CLEANUP-01..04, 07, 08)

- **Explicit classes.** `NetworkScope { class: ScopeClass, recorded, host_hint }` replaces the
  `client: bool`. The classes are `Host` (member/host), `Client` (member/client), `UnknownRole`
  (member/none, and fail-closed for anything the store should never yield), `CompletedLeave`
  (not-member/none) and `OrphanClientRecord` (a record with no membership row). The persisted
  role decides; a record only supplies a `Client` network's host hint and its provenance
  (`recorded`). Unreadable or invalid stores still fail before anything is removed.
- **Outcome.** An unresolved class is skipped: nothing is removed or disconnected, and the
  membership and record are not rewritten. It counts in the new `RemovalReport::unresolved_roles`,
  which fails `succeeded()` even when no owned resource remains. The helper log line gains
  `"unresolved_roles"` (a count; no device path or record content), and the exit status is 1.
- **Independent progress.** Other networks are still cleaned. The instance lock, removal marker,
  60-second bound, reobservation and existing client rules are unchanged.
- **Recovery.** Each run re-reads both stores. Bluetooth alone resolves nothing. After the role
  is established and stored, normal Client or Host cleanup finishes and repeats successfully.
  No role-editing API was added, and storage schema v3 and API 1.7 are unchanged. The documented
  manual recovery is the maintainer decision of 2026-11-14: confirm no PANU link, delete the
  owned profiles, then remove the stale record or membership lines by hand.
- **Packaged path.** The lifecycle harness gains an unresolved-role scenario. The **helper is
  mocked** (exit 1, as `cleanup_outcome` returns for this report). Remove, deconfigure and
  direct purge all stop at `prerm`; abort-remove/abort-deconfigure restart the service; the
  membership and client record survive; after a successful retry, purge deletes them.

### Regressions (LC-CLEANUP-05..07)

In `apps/daemon/src/removal.rs`; all 27 removal tests pass (20 earlier, 7 new):

| Test | Covers |
| --- | --- |
| `lc_cleanup_an_unknown_active_role_with_a_client_record_stays_unresolved` | The RFU-02 recipe through the production stores (LC-BASE-04) |
| `lc_cleanup_an_unknown_role_is_unresolved_in_every_combination` | Bluetooth off/on × record with hint, without hint, none. A confirmed host network beside it is cleaned and a foreign network is preserved. Helper exit 1 and the log field are asserted |
| `lc_cleanup_an_unknown_role_fails_even_with_no_owned_state_left` | Zero remaining resources is not success |
| `lc_cleanup_an_unknown_role_with_conflicting_proof_is_left_alone` | The owned profile names another interface; nothing is touched |
| `lc_cleanup_scope_classifies_every_stored_combination` | All 9 table rows through the production stores |
| `lc_cleanup_a_client_record_without_a_membership_is_unresolved` | Orphan record; a hint alone never disconnects |
| `lc_cleanup_recovery_needs_the_role_established_and_then_completes` / `lc_cleanup_a_recovered_host_role_finishes_with_host_cleanup` | Retry rereads; Bluetooth alone does not resolve; recovered Client and Host finish and repeat |

These earlier tests still pass: explicit host, explicit client, the NotMember stale-record case
(`fu_04_a_stale_client_record_does_not_block_removal`) and the unreadable-registry case.
Daemon workspace tests pass with no failures. The lifecycle harness passes with the
unresolved-role scenario.

### Negative controls (isolated worktree)

| Control | Result |
| --- | --- |
| RFU-02 baseline (`e167198`) | LC-BASE-04 test fails (see LC-BASE) |
| Fix with `member`/`none` classified as `CompletedLeave` | 7 of 27 fail: every unknown-role test, recovery and the table |
| Fix with an orphan record classified as `CompletedLeave` | 2 of 27 fail: the orphan test and the table |

## LC-INTEGRATION — Real boundaries, CI scheduling and sensitivity (2026-11-14)

**Source.** The LC-INTEGRATION commit on top of `0106e3a`.

### Consumers (LC-INTEGRATION-01)

| Boundary | Test | Observation |
| --- | --- | --- |
| Shared watcher (`blueroute_client::watch`) | `ownership_period::lc_integration_the_shared_watcher_treats_a_returning_owner_as_a_new_period` | The watcher's executor thread is blocked, using only acknowledgements, from the moment A receives the Version call until A has gone A→B→A and its reply is queued. Signals are exactly `[Connected, Disconnected, Connected]`, then `Changed` for A's new period. The old item is never passed on. The returning A is revalidated on reconnect (Versions 4 and 5) |
| Desktop (`watch_daemon`, re-run with the private bus as the system bus) | `watch_handover::the_desktop_watcher_reports_a_returning_daemon_as_a_new_period` | A→B→A inside the change's check. The desktop sees `Connected, Disconnected, Connected` and then the new period's `Changed` |
| TUI (`spawn_with`, real watcher thread) | `watch::tests::tui_watcher_reports_a_returning_daemon_as_a_new_period` | The same sequence through the TUI's channel |

The existing A→B handover tests for all three consumers still pass. The desktop file now
serializes its child scenarios with a lock, because two of them share the name.

### Cleanup through the production layers (LC-INTEGRATION-02)

- **Store → scope → cleanup → report → helper status.** The `lc_cleanup_*` tests seed the
  production membership and client-record stores. They run the production `removal_scope`,
  `run_removal_cleanup` (instance lock, removal marker, 60 s bound) and `cleanup_outcome`, and
  assert exit status 1 and the `unresolved_roles` field. **Mocked:** the NetworkManager/BlueZ
  backend (in-memory `Fake`). The root-only helper binary is not run, because it needs root and
  the system bus. Its `main` calls `cleanup_outcome` unchanged.
- **Packaged scripts.** `packaging/debian/test-maintainer-lifecycle.sh` runs the real
  `prerm`/`postrm`/`postinst` under a temporary `DPKG_ROOT`. Its unresolved-role scenario covers
  remove, deconfigure, abort, direct-purge stop, preserved state, retry and purge. **Mocked:**
  the packaged helper and `deb-systemd-*`.
- **Adapter.** The new unit test `pan::tests::a_missing_device_object_is_absent_and_other_failures_stay_errors`
  fixes the adapter half of the safe UnknownObject behavior. A missing device object (UnknownObject,
  UnknownInterface, DoesNotExist) reports `panu_device_present = false`, which cleanup treats as
  inconclusive (`fu_04_a_missing_host_device_is_inconclusive_even_with_bluetooth_running`).
  AccessDenied, NoReply, ServiceUnknown, Failed and I/O stay errors, which cleanup counts as a
  failed network. Neither becomes "link gone".

### CI scheduling (LC-INTEGRATION-03)

`.github/workflows/ci.yml` runs `ownership_period` with `--ignored` (added in LC-OWNER).
`watch_handover` runs with `--ignored`, and the TUI filter `tui_watcher` matches the new test's
name. The removal, client and adapter unit tests run in `cargo test --workspace`. The lifecycle
harness runs both from source and against the built package's extracted scripts.

### Negative controls (LC-INTEGRATION-04)

The controls were re-run in an isolated worktree with **its own target directory**. An earlier
attempt shared the main target directory, and cargo then reused the control's build in the main
tree for one run; the main-tree results were re-taken after forcing a rebuild. The script is
recorded in the session, not the repository. The fixed tree was the sanity baseline: 10/10
ownership tests and 27/27 removal tests passed.

| Control | Result |
| --- | --- |
| O1: ordered decision drain disabled | 6 of 10 ownership tests fail, including the shared-watcher integration test |
| O2: baseline `75c1a01` subscription code | The same 6 fail. The shared watcher reports `[Connected, Changed, Disconnected, Connected]`: the old item reaches the consumer |
| C1: `member`/`none` classified as completed leave | 7 of 27 removal tests fail |
| C2: orphan record classified as completed leave | 2 of 27 fail |
| C3: `unresolved_roles` ignored by `succeeded()` | 1 fails (`lc_cleanup_an_unknown_role_fails_even_with_no_owned_state_left`). In the other cases the untouched owned state still fails the run as `remaining`; the zero-resource test exists for this case |

The desktop and TUI returning-owner tests pass under O2. Their watchers poll continuously, so
the ownership notice always wins against the reply. They guard the consumer contract, and the
blocked-executor shared-watcher test is the sensitive one at that layer.

### Preserved behavior (LC-INTEGRATION-05)

At the LC-INTEGRATION source:

- All 15 CI broker suites pass: 61 tests across dbus_service, authorization, dbus_client,
  owner_race, event_owner_race, ownership_period, state_events, publisher_progress, owner_watch,
  watcher, watch_integration, watch_handover, tui_watcher, cli_exit_contract and
  bridge_end_to_end. These cover publisher progress and shutdown, startup handoff, ordinary
  owner handover and security boundaries.
- `cargo test --workspace --locked`: 911 passed, 0 failed, 62 ignored. This includes graph
  import/atomicity, client cleanup/retry and the NotMember stale-record case.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo fmt --check` are
  clean, and the lifecycle harness passes.
- The frontend (UI effect generation) is unchanged; it is re-run in LC-VALIDATE.

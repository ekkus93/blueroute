# Code Review Follow-up TODO

**Status:** IN PROGRESS. **Evidence:** [CODE_REVIEW_FOLLOWUP_EVIDENCE.md](CODE_REVIEW_FOLLOWUP_EVIDENCE.md).  
**Baseline:** `6d200d0cda24acd00b1f3a6b8981b1d0bb2d6eec`, reviewed 2026-11-13.  
**Companion specification:** [CODE_REVIEW_FOLLOWUP_SPEC.md](CODE_REVIEW_FOLLOWUP_SPEC.md).  
**Earlier backlog and evidence:** [CODE_REVIEW_REMEDIATION_TODO.md](CODE_REVIEW_REMEDIATION_TODO.md),
[CODE_REVIEW_REMEDIATION_EVIDENCE.md](CODE_REVIEW_REMEDIATION_EVIDENCE.md).  
**Original acceptance gates:** [TODO.md](TODO.md),
[HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md).

## Tracking and execution rules

- Use stable `FU-*` IDs in commits, PRs, tests, and evidence. Preserve earlier `CR-*` and `P*`
  identities; this backlog supplements them rather than silently resetting their history.
- `[ ]` pending, `[-]` in progress, `[x]` accepted, `[!]` blocked with a stated reason.
  The register below is the current follow-up task authority. Keep it consistent with subtasks.
- A checkbox requires implementation plus acceptance evidence. Parent acceptance applies in
  addition to its leaves. Document creation and a green existing suite do not establish a fix.
- Record evidence in a versioned record linked from the ledger below: exact SHA, source/tests,
  requirement/finding IDs, commands/results, negative controls, and limitations. Do not depend
  on temporary review artifacts. Create that evidence record during implementation.
- Run software work independently of deferred equipment. Preserve installed-system/physical
  gates and distinguish software completion from full qualification. Do not run qualification
  against ordinary host services, package state, or Bluetooth hardware as part of unit testing.
- Keep production join closed, sharing disabled, TUI read-only, and security/API/schema
  invariants intact. Use pinned toolchains and repository contribution instructions.

## Maintainer decisions (2026-11-13)

- **FU-04-02 proof retention:** keep the existing NetworkManager ownership proof (owned profile
  and address) until the dependent client PANU teardown is verified, and fail cleanup while it is
  unresolved. No new durable cleanup record and no storage/schema change.
- **CI evidence:** exact-revision CI is inspected with the GitHub CLI (`gh run`), recording run
  IDs. References to "Ralph Bridge" below and in the specification are satisfied this way.

## Task register and dependencies

| Task | Outcome / issue | Dependencies | State |
| --- | --- | --- | --- |
| FU-BASE | Durable baseline, corrected regressions, isolation | None | `[x]` |
| FU-01 | P2 REV-01: independent publication and bounded shutdown | FU-BASE | `[x]` |
| FU-02 | P2 REV-02: ownership monitored through validation | FU-BASE | `[x]` |
| FU-03 | P2 REV-03: safe segment cloning/import | FU-BASE | `[x]` |
| FU-04 | P2 REV-04: unresolved cleanup and durable retry proof | FU-BASE | `[x]` |
| FU-05 | GAP-01: startup snapshot/publication handoff | FU-01 design; FU-02 for reconnect integration | `[x]` |
| FU-06 | GAP-02: adapter-free qualification fixture and clear isolation | FU-04 contract; procedure work can start earlier | `[ ]` |
| FU-INTEGRATION | Boundary regressions and negative controls | FU-01..05 | `[ ]` |
| FU-VALIDATE | Exact-candidate software, packages, CI | FU-INTEGRATION; FU-06 software/fixture work | `[ ]` |
| FU-QUALIFY | Existing installed-system/physical gate evidence | FU-04/06; validated packages; designated equipment | `[ ]` |
| FU-CLOSEOUT | Consistent status and evidenced closure | Software: FU-01..06, FU-INTEGRATION/VALIDATE; full: FU-QUALIFY | `[ ]` |

Implement FU-01..04 independently after baseline preparation. Close the startup handoff with
the publication design, then integrate consumers and lifecycle tests. Equipment is needed for
FU-QUALIFY, not for the other software tasks or an explicitly limited software milestone.

## FU-BASE — Preserve the review and establish failing expectations

**Requirements:** ACCEPT-1/2/3 and the finding register in the specification.

- [x] **FU-BASE-01:** Record current HEAD, dirty state, toolchains, and differences from the
  review baseline. Read contributor guidance and current product/remediation requirements.
- [x] **FU-BASE-02:** Preserve REV-01..04 recipes and GAP-01..02 scope in versioned evidence,
  including the distinction between reproduced bugs and documented/qualification gaps.
- [x] **FU-BASE-03:** Add corrected expectations for slow sampling, event-time owner handover,
  cloned graph membership, and unavailable-Bluetooth cleanup/retry. Confirm baseline failures.
- [x] **FU-BASE-04:** Establish fixtures with barriers, bounded waits, known ownership, foreign
  resources, and pending operations. Ensure shutdown of test fixtures remains controllable.
- [x] **FU-BASE-05:** Verify private broker, temporary filesystem/package-root, and fake backend
  isolation. Prevent a real maintenance helper from reaching host services in package tests.

**Acceptance:** Every reproduced defect has a meaningful failing corrected expectation and a
durable recipe. Setup/teardown is isolated and does not confuse mocks with installed-system proof.

## FU-01 — Separate sampling, emission, and shutdown

**Requirements:** PUB-1..4. **Earlier tasks:** CR-F1-04/06/09, CR-INTEGRATION-03.

- [x] **FU-01-01:** Document bounded sampler/emitter responsibilities, observation ordering,
  dirty revision ownership, adapter deadlines, cancellation, and shutdown behavior.
- [x] **FU-01-02:** Remove pending network/storage observation from the signal delivery path,
  including initial sampling; keep slow work off shared publication locks.
- [x] **FU-01-03:** Retain the latest dirty state across sampling/emission races, queue
  saturation, and emission failures. Reject obsolete results and false empty-success snapshots.
- [x] **FU-01-04:** Bound sampler concurrency and terminate workers/tasks on shutdown without
  unbounded joins, abandoned blocking threads, or accumulated background work.
- [x] **FU-01-05:** On a private broker, hold production list observation pending and publish
  actual health/capability state. Require invalidation within one second before releasing it.
- [x] **FU-01-06:** Cover startup pending reads, sample failure/recovery, emitter failure/retry,
  bursts/final-state delivery, unchanged samples, and shutdown within two seconds in the fixture.

**Acceptance:** REV-01 fails with the baseline and passes with independent notification progress;
all four PUB requirements hold without sacrificing bounded work or committed-operation semantics.

## FU-02 — Retire subscriptions during event-time Version validation

**Requirements:** OWNER-1..4. **Earlier tasks:** CR-F2-01/02/06/07, CR-F8-05, CR-INTEGRATION-01.

- [x] **FU-02-01:** Define lifecycle and delivery decision ordering for signal selection,
  Version validation, queued owner transitions, cancellation, and terminal retirement.
- [x] **FU-02-02:** Monitor owner changes while Version is pending and verify the owner again
  before accepting an event; do not depend on another `next_event` call to notice retirement.
- [x] **FU-02-03:** Make a retired subscription reject all later A events/replies. Preserve
  typed failure, unique-owner requests, mutation no-replay, and bounded watcher backoff.
- [x] **FU-02-04:** Transfer A to compatible B inside A's event-time Version handler; keep A
  connected and require rejection of the selected obsolete event and queued A events.
- [x] **FU-02-05:** Hold that Version response after transfer. Require owner-loss reporting
  within two seconds, then release the late reply and verify retirement remains terminal.
- [x] **FU-02-06:** Cover incompatible B, no replacement, cancellation, and fresh subscription.
  Verify B receives no ordinary Request when incompatible and no mutation is replayed.
- [x] **FU-02-07:** Exercise shared watcher, desktop, and TUI recovery: visibly stale retained
  state, bounded reconnect, compatible B negotiation, and fresh snapshots rather than A replay.

**Acceptance:** Both REV-02 manifestations fail against the baseline and pass with the fix.
Command-owner safety and existing idle owner-loss behavior remain intact.

## FU-03 — Validate every accepted segment representation

**Requirements:** SEG-1..4. **Earlier tasks:** CR-F3-04/05/06, CR-INTEGRATION-05.

- [x] **FU-03-01:** Choose/document normalization or rejection for cloned/imported derived
  membership; preserve explicit membership and existing boolean/identity contracts.
- [x] **FU-03-02:** Validate NAP/explicit nodes and derive stored membership only from valid
  destination-graph evidence. Handle mutation of a cloned segment's public NAP field.
- [x] **FU-03-03:** Add cross-graph clone tests with a missing client and with the client
  present but no corresponding destination link. Check the entire graph after insertion.
- [x] **FU-03-04:** Test changed/unknown NAP, unknown explicit member, identical reinsertion,
  explicit non-link membership, property-only updates, and multiple-link removal.
- [x] **FU-03-05:** Assert whole-graph equality after rejection and run a downstream topology/
  forwarding calculation proving imported phantom membership cannot affect its result.

**Acceptance:** No successful public insertion can violate SEG-2; rejected operations are atomic.
The clone reproduction fails on the baseline and passes under the documented import policy.

## FU-04 — Keep unresolved client cleanup failed and retryable

**Requirements:** REMOVE-1..5. **Earlier tasks:** CR-F5-04/06/09, CR-INTEGRATION-06.

- [x] **FU-04-01:** Define observed-absent versus unresolved PANU outcomes and their ownership
  requirements; include unresolved work in cleanup success/reporting.
- [x] **FU-04-02:** Choose proof retention or a durable verified cleanup record. Document
  provenance, lifetime, recovery, foreign-state checks, and any required storage migration.
- [x] **FU-04-03:** Preserve dependent proof until client teardown is verified; continue safe
  independent NetworkManager work and cleanup of other known networks after partial failure.
- [x] **FU-04-04:** Correct the fixture that accepts success without Bluetooth. Require
  failure, unresolved work reporting, retained proof, and preserved unrelated foreign state.
- [x] **FU-04-05:** Restore Bluetooth in the same scenario and prove the retry disconnects the
  owned link or verifies absence; repeat to prove idempotence after complete cleanup.
- [x] **FU-04-06:** Test missing/conflicting proof, reused interface names, foreign attachments,
  already absent links, interrupted steps, backend failure, timeout, and host-only/empty state.
- [x] **FU-04-07:** Exercise the actual helper/report and maintainer/isolated-dpkg paths:
  unresolved cleanup fails remove/deconfigure/purge, preserves retry evidence, and recovers.
  Confirm upgrades and abort paths still follow their distinct lifecycle contract.
- [x] **FU-04-08:** Preserve instance locking, removal-marker quiescence, bounded non-secret
  diagnostics, and the 60-second run limit. Do not add production shell networking commands.
- [x] **FU-04-09:** Revise the skip-and-succeed cleanup design and associated software evidence;
  explicitly retain CR-QUALIFY and original installed-system/physical acceptance gates.

**Acceptance:** The unavailable-Bluetooth reproduction cannot report success or destroy the last
retry proof. A recovered retry resolves the owned attachment without broadening ownership scope.

## FU-05 — Remove the startup baseline notification gap

**Requirements:** START-1..3, PUB-4. **Earlier tasks:** CR-F1, CR-INTEGRATION; EVENT-5.

- [x] **FU-05-01:** Define service availability, baseline capture, dirty revision, initial
  subscription, and snapshot ordering; implement a handoff that cannot silently absorb a change.
- [x] **FU-05-02:** Force a change between a client's initial read and baseline initialization
  with barriers; require the latest snapshot or a timely invalidation without manual emission.
- [x] **FU-05-03:** Repeat while initial network sampling is pending and during compatible
  owner replacement, preserving subscribe/read reconciliation and bounded notification work.
- [x] **FU-05-04:** Update the earlier documented startup limitation with the fix and evidence;
  do not claim exactly-once delivery or faster physical observation intervals.

**Acceptance:** Deterministic tests demonstrate the formerly missed ordering and verify that
neither startup nor reconnect leaves a connected client permanently on the old snapshot.

## FU-06 — Repair qualification prerequisites and isolation claims

**Requirements:** QUAL-1..4. **Earlier tasks:** CR-QUALIFY-01..05.

- [ ] **FU-06-01:** Replace Part A's adapter-dependent fixture with an explicit test-only owned
  NetworkManager/registry setup; document exact ownership/schema, permissions and teardown.
- [ ] **FU-06-02:** Specify adapter-free VM dependencies, isolated dummy/bridge devices,
  foreign-state comparisons, and independent kernel/backend observations for each case.
- [ ] **FU-06-03:** Separate VM NetworkManager cleanup from physical client/PAN teardown.
  Assign unfaithful VM cases to Part B; seeded unresolved client state must fail honestly.
- [ ] **FU-06-04:** Audit test harness/helper/bus isolation and document the `DPKG_ROOT`
  limitation. Keep fixture tools outside production paths and ownership authorization.
- [ ] **FU-06-05:** Update the procedure's evidence checklist, expected statuses/log fields,
  equipment requirements, and links; distinguish procedure readiness from executed qualification.

**Acceptance:** Part A has an actionable adapter-free path with honest scope. Every physical
claim has an explicit equipment/evidence gate; path relocation is never called bus isolation.

## FU-INTEGRATION — Verify production boundaries and regression sensitivity

**Requirements:** ACCEPT-1; PUB-4, OWNER-4, SEG-4, REMOVE-5, START-3.

- [ ] **FU-INTEGRATION-01:** Connect real service producers, private-broker invalidation, shared
  watcher, and snapshot refresh while network sampling is stalled, including cold startup.
- [ ] **FU-INTEGRATION-02:** Carry event-time owner replacement through desktop/TUI consumers
  and fresh-state recovery; verify incompatible-owner refusal and no mutation replay.
- [ ] **FU-INTEGRATION-03:** Exercise imported graph state through a downstream consumer and
  unresolved cleanup through the packaged failure/retry path, not only helper predicates.
- [ ] **FU-INTEGRATION-04:** Add relevant tests to CI, explicitly scheduling ignored broker
  tests on private buses; identify any intentionally ignored/environment-dependent checks.
- [ ] **FU-INTEGRATION-05:** Run safe negative controls for REV-01..04 and GAP-01 and record
  expected failures. Confirm fixtures actually exercise the real code being corrected.

**Acceptance:** Each software defect has a production-path regression sensitive to its root cause.
No required test silently disappears behind `#[ignore]` or only asserts a fixture's behavior.

## FU-VALIDATE — Qualify the exact software candidate

**Requirements:** ACCEPT-2, preserved security/platform invariants.

- [ ] **FU-VALIDATE-01:** Run locked workspace build/check/tests, formatting, and strict
  all-target Clippy using pinned Rust. Record actual test counts, failures, and ignored cases.
- [ ] **FU-VALIDATE-02:** Run daemon `experimental-join` tests and all existing/new private-bus
  suites; run Python socket-tooling tests. Verify ordinary builds keep production join closed.
- [ ] **FU-VALIDATE-03:** With pinned Node/npm, run frontend install, format/lint/type checks,
  component/StrictMode tests, and production build; retain effect-generation isolation coverage.
- [ ] **FU-VALIDATE-04:** Build both Debian packages and inspect manifests/dependencies/assets.
  Run maintainer lifecycle, state migration, isolated upgrade, failure/retry, remove/purge checks.
- [ ] **FU-VALIDATE-05:** Recheck authorization, owner binding/no replay, authenticated identity,
  revocation, rollback, foreign-resource preservation, stable identity/schema, and disabled sharing.
- [ ] **FU-VALIDATE-06:** Record candidate SHA, commands/results, toolchains and package hashes;
  inspect exact-revision CI through Ralph Bridge. Attribute earlier results to their earlier SHA.

**Command reference (repository root unless stated):**

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p blueroute-daemon --features experimental-join --locked
dbus-run-session -- cargo test -p blueroute-daemon --test state_events --locked -- --ignored
dbus-run-session -- cargo test -p blueroute-client --test owner_race --locked -- --ignored
dbus-run-session -- cargo test -p blueroute-client --test owner_watch --locked -- --ignored
dbus-run-session -- cargo test -p blueroute-client --test watcher --locked -- --ignored
python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v
bash packaging/debian/test-maintainer-lifecycle.sh
bash packaging/debian/build-package.sh
cargo build --release --locked -p blueroute-linux --example state_check
```

In `apps/desktop`, run `npm ci`, `npm run format:check`, `npm run lint`,
`npm run typecheck`, `npm test`, and `npm run build`. Also run the remaining existing broker
checks in [ci.yml](../.github/workflows/ci.yml): daemon service/authorization, client D-Bus,
desktop watch/bridge, TUI watcher, and CLI exit contracts, plus new suites added by this work.

Run `bash packaging/debian/test-package-upgrade.sh <fresh-core-package.deb>
target/release/examples/state_check` with the exact newly built core package path. This is the
isolated harness; installing/removing a package on the host is not a substitute.

**Acceptance:** All required software checks and exact-candidate CI pass; evidence states actual
coverage and limitations without claiming installed-system/physical qualification.

## FU-QUALIFY — Preserve and satisfy the existing environment gates

**Requirements:** QUAL-2/4, ACCEPT-4. **Authoritative existing criteria:** CR-QUALIFY-01..08.

- [ ] **FU-QUALIFY-01:** On designated disposable equipment, execute revised Part A and record
  actual NetworkManager/kernel/systemd/dpkg results for CR-QUALIFY-01..05.
- [ ] **FU-QUALIFY-02:** On a designated Bluetooth rig, execute Part B for CR-QUALIFY-06..07,
  including unavailable/restored Bluetooth, owned-client retry, and foreign-state preservation.
- [ ] **FU-QUALIFY-03:** Store reproducible versioned evidence with exact candidate/package
  identity, hardware/software inventory, commands and independent before/after observations;
  link CR-QUALIFY-08 and the hardware tracker without generalizing support.
- [ ] **FU-QUALIFY-04:** If equipment remains deferred, record the blocker and leave this task
  and all unsatisfied original criteria open. Documentation readiness is not qualification.

**Acceptance:** All applicable original CR-QUALIFY criteria are evidenced. A deferral note can
complete the reporting subtask, but cannot complete this parent or imply physical acceptance.

## FU-CLOSEOUT — Reconcile completion claims and review the result

**Requirements:** ACCEPT-3/4 and every requirement's evidence mapping.

- [ ] **FU-CLOSEOUT-01:** Review every REV-01..04 fix and GAP-01..02 outcome against the
  specification, baseline reproduction, adverse cases, negative controls, and security boundaries.
- [ ] **FU-CLOSEOUT-02:** Reconcile earlier CR-F1/F2/F3/F5/F8, integration and closeout claims
  with dated corrections and links. Preserve historical evidence and earlier verified behavior.
- [ ] **FU-CLOSEOUT-03:** Update affected product/current-status summaries, cleanup design and
  qualification procedure. Preserve P0–P19 identities and all unsatisfied product/hardware gates.
- [ ] **FU-CLOSEOUT-04:** Fill the evidence ledger and map every specification requirement to
  implementation/tests or qualification evidence; check all links, dependencies and task states.
- [ ] **FU-CLOSEOUT-05:** Record software closure only when its criteria pass; record full
  remediation closure only after FU-QUALIFY and all remaining acceptance gates pass. Otherwise
  use SOFTWARE COMPLETE / QUALIFICATION DEFERRED and keep the parent open.

**Acceptance:** All findings/gaps have evidenced outcomes, current claims are consistent, and
the overall completion label reflects remaining qualification rather than merely passing CI.

## Evidence ledger

Replace placeholders with versioned links as work is accepted. Record a software milestone
separately from the unresolved qualification gate.

| Task / issue | Fix SHA and source | Regression / negative-control / software evidence | Qualification or remaining gate | State |
| --- | --- | --- | --- | --- |
| FU-BASE | Tests at this commit | 5 failing corrected expectations ([evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md#fu-base--baseline-and-failing-expectations-2026-11-13)) | None | COMPLETE |
| FU-01 / REV-01 | `19a58a0` | 5 `publisher_progress` tests (REV-01 failed at baseline); [evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md#fu-01--independent-publication-and-bounded-shutdown-2026-11-13) | None | COMPLETE |
| FU-02 / REV-02 | `d2b94c1` | 6 `event_owner_race` tests (both REV-02 cases failed at baseline); [evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md#fu-02--ownership-monitored-through-validation-2026-11-13) | None | COMPLETE |
| FU-03 / REV-03 | `1f2d48d` | 4 `fu_03` tests (REV-03 failed at baseline); [evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md#fu-03--safe-segment-cloningimport-2026-11-13) | None | COMPLETE |
| FU-04 / REV-04 | `718983c` | 17 removal tests (REV-04 failed at baseline); [evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md#fu-04--unresolved-client-cleanup-fails-and-stays-retryable-2026-11-13) | FU-QUALIFY for actual installed/PAN claims | SOFTWARE COMPLETE / QUALIFICATION DEFERRED |
| FU-05 / GAP-01 | `c104bea` | 2 forced-order tests (both fail without the handoff); [evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md#fu-05--startup-snapshotpublication-handoff-2026-11-13) | None | COMPLETE |
| FU-06 / GAP-02 | — | QUAL-1..4 procedure/fixture pending | Execution remains separate | NOT STARTED |
| FU-INTEGRATION | — | ACCEPT-1 pending | Boundary sensitivity and CI scheduling | NOT STARTED |
| FU-VALIDATE | — | ACCEPT-2 pending | Exact-candidate checks/packages/CI | NOT STARTED |
| FU-QUALIFY | — | Software results do not satisfy this row | CR-QUALIFY-01..08; designated equipment | NOT STARTED |
| FU-CLOSEOUT | — | ACCEPT-3/4 pending | Software milestone; full qualification separately | NOT STARTED |

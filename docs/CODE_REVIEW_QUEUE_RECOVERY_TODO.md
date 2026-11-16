# Code Review Queue and Recovery Remediation TODO

**Status:** SOFTWARE COMPLETE / QUALIFICATION DEFERRED (2026-11-14, candidate `b700577`); QR-QUALIFY-02 blocked on equipment. **Evidence:** [CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md).  
**Baseline:** `f957d80207930ccb481bec8f23cefd8bb2506cde`.  
**Companion specification:** [CODE_REVIEW_QUEUE_RECOVERY_SPEC.md](CODE_REVIEW_QUEUE_RECOVERY_SPEC.md).  
**Later work:** RQR-01 (bounded `reconnect` timeout, outside the QR scope) is tracked in
[CODE_REVIEW_RECONNECT_TODO.md](CODE_REVIEW_RECONNECT_TODO.md).  
**Earlier work:** [lifecycle backlog](CODE_REVIEW_LIFECYCLE_TODO.md),
[lifecycle evidence](CODE_REVIEW_LIFECYCLE_EVIDENCE.md), and [original backlog](TODO.md).

## Tracking rules

- Keep stable `QR-*` IDs alongside RLC-01/02 and existing LC/FU/CR/P references. The register
  below is the current status authority for this work; synchronize states and evidence.
- Use `[ ]` pending, `[-]` in progress, `[x]` accepted and `[!]` blocked with a reason. All items
  start pending; this document establishes no fix, validation result or qualification.
- Accept a task only after its behavior, regressions and parent-specific criteria pass. Record
  source SHA, commands/results, negative controls and limitations in versioned evidence created
  during implementation and linked from the ledger. Temporary review artifacts are insufficient.
- Continue independent software work during equipment deferral. Preserve qualification gates,
  original task identities, security/ownership rules, pinned tools and compatibility contracts.
- Keep existing unknown-role failure/proof retention. No new role-editing API, storage journal,
  wire epoch, frontend privilege, open production join or Internet sharing is required.

## Task register and dependencies

| Task | Outcome | Dependencies | State |
| --- | --- | --- | --- |
| QR-BASE | Durable recipes and corrected failing expectations | None | `[x]` |
| QR-QUEUE | P2 RLC-01: bounded-queue progress with ordered retirement | QR-BASE | `[x]` |
| QR-RECOVERY | P3 RLC-02: complete targeted manual recovery | QR-BASE | `[x]` |
| QR-INTEGRATION | Actual consumer/helper boundaries and sensitive CI tests | QR-QUEUE, QR-RECOVERY | `[x]` |
| QR-VALIDATE | Exact-candidate software/package/CI checks | QR-INTEGRATION | `[x]` |
| QR-QUALIFY | Preserve existing installed-system/physical gates | Procedure: QR-RECOVERY; execution: validated packages and designated equipment | `[!]` no designated equipment |
| QR-CLOSEOUT | Reviewed closure and current status reconciliation | Software: QR-QUEUE/RECOVERY/INTEGRATION/VALIDATE; full: QR-QUALIFY | `[-]` software closed |

QR-QUEUE and QR-RECOVERY can proceed independently after baseline preparation. Documentation and
qualification-procedure work can start with their designs. Equipment is not a dependency for the
two fixes, their software regressions, or an explicitly limited software milestone.

## QR-BASE — Preserve evidence and establish corrected expectations

**Requirements:** ACCEPT-1/2; finding recipes in the companion specification.

- [x] **QR-BASE-01:** Record current HEAD, dirty state, pinned toolchains and differences from
  the review baseline. Read current product/lifecycle/queue-recovery requirements and guidance.
- [x] **QR-BASE-02:** Preserve RLC-01/02 triggers, paths, results and prior task relationships
  in versioned evidence, including queue-capacity control and the distinction between safe
  cleanup refusal and incomplete administrator instructions.
- [x] **QR-BASE-03:** Add corrected above-capacity owner-loss/shared-connection expectations and
  a production-store manual-recovery scenario; demonstrate intended baseline failures.
- [x] **QR-BASE-04:** Make fixtures finite, acknowledgement-controlled and isolated. Keep state /
  logs in ignored workspace `tmp/`; ensure private buses, helper mocks and teardown cannot reach
  host services, ordinary networking or the host package database.

**Acceptance:** Both findings have durable recipes and meaningful failing corrected expectations;
the queue control and incomplete-record case explain the failure rather than timing guesses.

## QR-QUEUE — Keep final verification and shared calls progressing

**Requirements:** QUEUE-1..6. **Earlier tasks:** LC-OWNER-02/05/06, LC-INTEGRATION-01.

- [x] **QR-QUEUE-01:** Document all subscription-owned queues, capacities, receive-sequence
  ordering, terminal/cancellation state and lifetime. Identify how every receiver progresses
  while signal selection, Version and final verification are awaiting results.
- [x] **QR-QUEUE-02:** Remove the circular wait between unread owner notices and the final
  GetNameOwner reply. Handle already queued period ends before another required round trip and
  maintain progress during final verification/error paths with bounded work.
- [x] **QR-QUEUE-03:** Preserve pre-decision rejection, post-decision permitted delivery and
  persistent next-call retirement. Keep returning owners bound only through fresh subscriptions;
  distinguish transient verification failures from confirmed loss.
- [x] **QR-QUEUE-04:** Release or safely settle receivers on terminal retirement/drop, including
  when the retired handle remains alive. Preserve other active subscriptions and cancellation
  state without unbounded tasks/buffers, detached drainers or busy polling.
- [x] **QR-QUEUE-05:** Test below/at/above-capacity finite bursts for Event and StateChanged,
  including 40 release/reacquire cycles after a ready Version reply. Require OwnerLost within
  two seconds of consumer resumption and no obsolete item.
- [x] **QR-QUEUE-06:** Assert a second call on the shared connection completes after retirement
  while the old handle is retained, and after drop. Cover cancellation/resumption, fresh
  subscription, unrelated active subscription preservation and bounded teardown.
- [x] **QR-QUEUE-07:** Add direct pre-/post-decision notice coverage, using a focused internal
  seam/state model where real-broker ordering cannot place the window deterministically. Keep
  the actual transport saturation test and document the boundary of each test/proof.
- [x] **QR-QUEUE-08:** Re-run ordinary A→B→A/A→unowned→A, held/late reply, cancellation, healthy
  owner and transient-error cases. Preserve unique-owner commands, compatibility, zero Requests
  to incompatible successors, no mutation replay and bounded reconnect behavior.

**Acceptance:** RLC-01 fails at baseline and passes at saturation with ordered retirement and
shared-connection progress. Larger capacity or caller-side forced drop alone is not the fix.
The formerly unexercised post-decision boundary has direct evidence.

## QR-RECOVERY — Align guidance and test the complete record deletion

**Requirements:** RECOVER-1..4. **Earlier tasks:** LC-CLEANUP-07, LC-CLOSEOUT-01..05.

- [x] **QR-RECOVERY-01:** Align ADMINISTRATOR.md with qualification Part A: after independently
  verified PAN/NM teardown, remove the targeted unknown membership/peer rows and any client
  record naming the same full NetworkId. Preserve the service-stopped/writer-quiesced ordering.
- [x] **QR-RECOVERY-02:** Explain no-matching-record, unrelated-record, orphan-record and
  confirmed completed-leave cases. Preserve foreign resources/records and fail unresolved
  preconditions; do not suggest guessing roles or clearing unrelated state.
- [x] **QR-RECOVERY-03:** Seed Member/None plus matching record through production stores.
  Verify membership-only deletion still fails as OrphanClientRecord; the completed documented
  sequence must remove the target obligation and produce helper exit 0.
- [x] **QR-RECOVERY-04:** Test no matching record, a record for another network, existing orphan,
  unrelated memberships/peers and completed-leave behavior. Verify retries, idempotent success
  and unchanged foreign-resource counts after independently completed resource cleanup.
- [x] **QR-RECOVERY-05:** Retain tests that restore an independently verified Host/Client role
  and all automatic proof-retention/failure behavior. Do not add a production role-editing API,
  schema change or automatic record deletion to make manual recovery pass.
- [x] **QR-RECOVERY-06:** Reconcile administrator guide, cleanup design, qualification procedure
  and current lifecycle evidence/summaries. State the manual preconditions, exact record scope,
  expected retry outcome and distinction from executed qualification consistently.

**Acceptance:** The original combined-state recovery succeeds using the documented complete
sequence. The deliberately incomplete sequence remains safely failed; unrelated state survives.
Guide and qualification procedure agree without weakening cleanup classification.

## QR-INTEGRATION — Verify the actual boundaries and regression sensitivity

**Requirements:** ACCEPT-1/2; QUEUE-5/6, RECOVER-3/4.

- [x] **QR-INTEGRATION-01:** Carry saturated ownership loss through the production shared
  watcher and desktop/TUI adapters. Verify disconnected/stale before fresh connection/change,
  timely notification and no old payload; identify which tests expose the baseline defect.
- [x] **QR-INTEGRATION-02:** Exercise production stores→scope→cleanup→helper outcome for the
  complete manual record transition, then compose maintainer/isolated-dpkg failure, retained
  evidence, retry and purge checks. Identify every mocked backend or installed helper layer.
- [x] **QR-INTEGRATION-03:** Run negative controls safely in isolated checkout/target directories:
  the old final-await behavior must fail saturation, and the old recovery sequence must fail
  completed-recovery acceptance. Retain passing below-capacity and incomplete-record controls.
- [x] **QR-INTEGRATION-04:** Schedule every new required regression in CI, including explicit
  private-bus invocations for ignored tests. Verify filters include the intended consumer cases.
- [x] **QR-INTEGRATION-05:** Re-run preserved publisher progress/shutdown/startup, graph import/
  atomicity, normal owner handover, unknown-role proof retention, stale-record cleanup, UI effect
  generation, authorization, pins/revocation, rollback, no-replay and closed join/sharing checks.

**Acceptance:** Both fixes have sensitive maintained production-path regressions and accurate
layer attribution. Existing green suites and helper mocks alone cannot close the reported gaps.

## QR-VALIDATE — Validate the exact software candidate

**Requirements:** ACCEPT-2; preserved security, platform and compatibility invariants.

- [x] **QR-VALIDATE-01:** Run pinned Rust formatting, locked workspace build/check/tests and
  strict all-target Clippy, plus experimental-join software tests. Confirm default builds keep
  join closed; record actual passed/failed/ignored counts.
- [x] **QR-VALIDATE-02:** Run all existing/new private-broker suites and Python socket-tooling
  tests; record commands, environment and any ignored or unavailable cases explicitly.
- [x] **QR-VALIDATE-03:** With pinned Node/npm, run frontend clean install, format/lint/type
  checks, component/StrictMode tests and production build; preserve narrow Tauri permissions.
- [x] **QR-VALIDATE-04:** Build both Debian packages and inspect manifests/dependencies/assets.
  Run source/extracted packaged-script lifecycle tests, state migration, isolated upgrade,
  cleanup failure/retry, remove and purge checks against the fresh candidate.
- [x] **QR-VALIDATE-05:** Record exact SHA, toolchains, commands/results, package hashes,
  preserved-invariant results and limitations. Inspect exact-revision CI with repository-approved
  tooling and attribute earlier candidate results to their own source revisions.

**Command reference:** from the repository root, with Rust 1.95.0:

```bash
cargo fmt --all -- --check
cargo build --workspace --locked
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p blueroute-daemon --features experimental-join --locked
dbus-run-session -- cargo test -p blueroute-client --test ownership_period --locked -- --ignored
cargo test -p blueroute-daemon --lib --locked removal
python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v
bash packaging/debian/test-maintainer-lifecycle.sh
bash packaging/debian/build-package.sh
cargo build --release --locked -p blueroute-linux --example state_check
```

Also run the remaining broker suites in [ci.yml](../.github/workflows/ci.yml) and new suites
added by QR-INTEGRATION, explicitly executing ignored cases under `dbus-run-session`.
In `apps/desktop`, use Node 26.13.0/npm 11.20.0: `npm ci`, `npm run format:check`, `npm run lint`,
`npm run typecheck`, `npm test` and `npm run build`.

Run `bash packaging/debian/test-package-upgrade.sh <fresh-core-package.deb>
target/release/examples/state_check`. Extract the fresh package's control scripts to an ignored
temporary directory and set `BLUEROUTE_MAINTAINER_SCRIPT_DIR` to that directory when running
`test-maintainer-lifecycle.sh`. Keep isolated package files and logs under workspace `tmp/`;
`DPKG_ROOT` relocates paths, not bus connections, so retain helper/systemd mocks in that harness.

**Acceptance:** Required software checks and exact-candidate CI pass with actual evidence. The
review's earlier 911 Rust, 63 broker and 43 frontend passes are historical baselines, not closure.

## QR-QUALIFY — Preserve the existing environment gates

**Requirements:** ACCEPT-4; existing LC-QUALIFY/FU-QUALIFY/CR-QUALIFY and hardware criteria.

- [x] **QR-QUALIFY-01:** Reconcile the installed-system recovery procedure with the corrected
  administrator sequence, matching-record/foreign comparisons and candidate identity. Preserve
  its complete two-record deletion case and distinguish procedure readiness from execution.
- [!] **QR-QUALIFY-02:** *(blocked: needs a disposable Debian 13 VM with sudo for Part A and the designated Bluetooth rig for Part B; the kit is built at `b700577`)* On designated disposable equipment, execute the applicable recovery /
  remove/retry/purge acceptance and record real backend/kernel/systemd/dpkg observations. Retain
  the separate Part B physical PAN gate and link results through existing qualification trackers.
- [x] **QR-QUALIFY-03:** If equipment remains unavailable, record deferral and keep this parent
  and all unmet original installed-system/physical criteria open; continue independent software.

**Acceptance:** Applicable environment criteria have actual evidence. A revised procedure or
completed deferral-reporting leaf cannot satisfy execution or complete this parent.

## QR-CLOSEOUT — Review and reconcile all outcomes

**Requirements:** ACCEPT-3/4 and all requirement-to-evidence mappings.

- [x] **QR-CLOSEOUT-01:** Review both fixes against reproduction, saturation/control results,
  delivery-decision/cancellation semantics, complete manual workflow and preserved boundaries.
- [x] **QR-CLOSEOUT-02:** Append dated corrections to LC-OWNER/LC-CLEANUP/LC-INTEGRATION/
  LC-CLOSEOUT software claims and related evidence; preserve history and LC/FU/CR/P identities.
  Link this backlog as the current RLC-01/02 status authority.
- [x] **QR-CLOSEOUT-03:** Reconcile product/current summaries, client queue/lifecycle contract,
  administrator recovery, cleanup design and qualification procedure. Preserve every unsatisfied
  original product/hardware gate and the existing safe unknown-role policy.
- [x] **QR-CLOSEOUT-04:** Complete versioned evidence mapping all 14 requirements to tasks,
  source, corrected baseline failures, final tests/proofs and limitations; verify links and states.
- [-] **QR-CLOSEOUT-05:** *(software closure recorded at `b700577`; full closure waits for QR-QUALIFY)* Record software completion only after fixes, integration, validation,
  documentation and review pass. Keep full closure open until QR-QUALIFY and existing required
  gates pass; use SOFTWARE COMPLETE / QUALIFICATION DEFERRED where appropriate.

**Acceptance:** Both findings and the post-decision coverage gap have evidenced outcomes.
Completion labels distinguish software correction, procedure readiness and executed qualification.

## Evidence ledger

Populate as work is accepted. Create the versioned evidence record during implementation and
link exact revisions/results; keep software and real environment outcomes separate.

| Task / finding | Fix SHA / source | Regression, control and software evidence | Qualification or remaining gate | State |
| --- | --- | --- | --- | --- |
| QR-BASE | `e7720a9` baseline; `queue_saturation.rs`, recovery recipe | RLC-01: 6 of 9 fail at baseline, 64-notice boundary passes, 66 fails; RLC-02 recipe fails with `unresolved_roles: 1` ([evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-base--findings-preserved-and-corrected-expectations-established-2026-11-14)) | Durable isolated recipes | ACCEPTED |
| QR-QUEUE / RLC-01 | `crates/blueroute-client/src/lib.rs`; SPEC client paragraph | 13 saturation tests pass (10 runs), `decide` post-decision seam, zbus pin guard; all preserved suites pass ([evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-queue--bounded-queue-progress-with-ordered-retirement-2026-11-14)) | Saturation, shared progress, post-decision evidence | ACCEPTED |
| QR-RECOVERY / RLC-02 | ADMINISTRATOR.md step 4/5 and table; CR-F5 design; CR-QUALIFY A3; no production code | 6 recovery tests (incomplete sequence fails as OrphanClientRecord, complete sequence exit 0, idempotent, unrelated state kept, step-3 negative control); step 5 blind spot found and documented ([evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-recovery--complete-targeted-manual-recovery-2026-11-14)) | Complete targeted manual workflow | ACCEPTED |
| QR-INTEGRATION | `e09f697`, `128d44e`, `c317716` | saturated loss at watcher/TUI/desktop; helper path and maintainer recovery walk; controls C1–C6 each fail their targets; suite scheduled in CI ([evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-integration--consumer-and-helper-boundaries-sensitivity-and-ci-2026-11-14)) | Actual consumers/helper boundaries and CI scheduling | ACCEPTED |
| QR-VALIDATE | `c317716`, revalidated `b700577` | 920 workspace, 392 join, 84 broker, 7 Python, 43 frontend; packages, upgrade, packaged scripts; [CI 43652001563](https://github.com/ekkus93/blueroute/actions/runs/43652001563) and [CI 43653522625](https://github.com/ekkus93/blueroute/actions/runs/43653522625) ([evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-validate--exact-software-candidate-2026-11-14)) | No installed-system/physical claim | COMPLETE |
| QR-QUALIFY | `0b58cef` procedure; kit at `b700577` | procedure reconciled; execution deferred ([evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-qualify--environment-gates-2026-11-14)) | Existing installed-system/physical gates | BLOCKED (execution) |
| QR-CLOSEOUT | `f17b87b` review fixes; `b700577` | [review](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-closeout-01--independent-review-of-f957d80c317716-2026-11-14) (7 items resolved), dated corrections, [requirement map](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md#qr-closeout--revalidation-requirement-map-and-software-milestone-2026-11-14); [CI 43653522625](https://github.com/ekkus93/blueroute/actions/runs/43653522625) success | Software milestone; full qualification separately | SOFTWARE COMPLETE / QUALIFICATION DEFERRED |

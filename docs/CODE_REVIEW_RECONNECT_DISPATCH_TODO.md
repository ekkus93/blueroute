# Code Review Reconnect Dispatch TODO

**Status:** IN PROGRESS; RCD-BASE, RCD-GUARD and RCD-TEST accepted.  
**Finding / priority:** RRC-01 / P2.  
**Baseline:** `0257bcbf11f37276771a477e612529fc3e10f155` (reviewed 2026-11-16).  
**Companion specification:** [CODE_REVIEW_RECONNECT_DISPATCH_SPEC.md](CODE_REVIEW_RECONNECT_DISPATCH_SPEC.md).  
**Implementation evidence:** [CODE_REVIEW_RECONNECT_DISPATCH_EVIDENCE.md](CODE_REVIEW_RECONNECT_DISPATCH_EVIDENCE.md).  
**Related work:** [previous reconnect backlog](CODE_REVIEW_RECONNECT_TODO.md),
[previous evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md), [previous specification](CODE_REVIEW_RECONNECT_SPEC.md),
and [original backlog](TODO.md), P5-005.

## Tracking and authority

- Use stable `RCD-*` task IDs and reference RRC-01, earlier `RC-*` tasks and P5-005 in evidence.
  This register tracks the remaining dispatch correction; it does not replace unrelated backlogs.
- `[ ]` pending, `[-]` in progress, `[x]` accepted, `[!]` blocked with a reason. Keep parent and
  child states consistent. Check items only after their acceptance criteria are evidenced.
- Earlier RC COMPLETE assertions do not establish RRC-01 acceptance. Preserve historical runs
  and working fixes while adding dated corrections; creating these two files completes no task.
- Use versioned evidence, maintained tests and exact revisions. Ignored temporary review artifacts
  may help investigation but must not become the only reproduction or execution dependency.
- Acceptance is software-only. Preserve existing hardware/installed-system/release gates as
  separate open work; continue this correction without a new equipment dependency.

## Task register and dependencies

| Task | Outcome | Dependencies | State |
| --- | --- | --- | --- |
| RCD-BASE | Durable reproduction, scope and corrected failing expectation | None | `[x]` |
| RCD-GUARD | Expiry guards before resumed work and new dispatch | RCD-BASE; design may proceed concurrently | `[x]` |
| RCD-TEST | Public and internal boundary matrix with sensitive controls | RCD-GUARD design; tests may precede code | `[x]` |
| RCD-VALIDATE | Exact-candidate software/package checks and CI | RCD-GUARD, RCD-TEST | `[ ]` |
| RCD-CLOSEOUT | Reviewed evidence, corrected claims and software closure | All preceding acceptance | `[ ]` |

## RCD-BASE — Preserve the finding and establish sensitive fixtures

**Requirements:** TEST-1/2/3, EVIDENCE-2/3.

- [x] **RCD-BASE-01:** Record current HEAD, dirty state, toolchains and differences from the
  reviewed baseline. Read contributor guidance, product/P5-005 requirements and both reconnect
  plans/evidence. Separate this remaining dispatch bug from the already-fixed RQR-01 failures.
- [x] **RCD-BASE-02:** Preserve the public 50 ms reproduction in versioned evidence: lookup
  dispatched, owner reply ready, future left unpolled past expiry, Version count one before
  resumption and two afterwards, despite typed timeout. Record three review failures/timely
  controls as historical evidence and distinguish resume time from post-result observation.
- [x] **RCD-BASE-03:** Add a maintained corrected public-method regression to
  `crates/blueroute-client/tests/reconnect_deadline.rs`: timeout on the resumed poll and zero
  reconnect Version calls. Run against the reviewed behavior and retain a timely-owner control.
- [x] **RCD-BASE-04:** Establish fixture phases through matched replies/acknowledgements and
  observable dispatch counts. Bound failure waits, release held work on error/panic, serialize
  name ownership where needed and use private brokers/ignored workspace `tmp/`. Do not silently
  skip a setup race or allow the fixture watchdog to stand in for the client deadline.

**Acceptance:** Correctness assertions fail for RRC-01 at the baseline and the timely control
passes. Durable instructions and maintained tests reproduce the issue without old review files.

## RCD-GUARD — Stop expired negotiation before it advances

**Requirements:** DISPATCH-1..5, PRESERVE-1..3.

- [x] **RCD-GUARD-01:** Define the dispatch decision point, first-poll budget, resumed-poll
  expiry priority and stage/outcome checks. Review `reconnect`, `negotiate_within`,
  `validated_owner`, `current_owner`, proxy setup and Version together; identify all relevant
  awaits and which normal request/subscription callers must retain their current behavior.
- [x] **RCD-GUARD-02:** Guard each resumed negotiation poll before advancing its inner future.
  If the deadline has expired, return the existing typed timeout on that poll without a new
  lookup, Version or retry. Remove dependence on attempt-first polling for timeout progress.
- [x] **RCD-GUARD-03:** Apply the same deadline at lookup/Version dispatch decisions, including
  when a poll starts before expiry but reaches another stage afterwards. Keep the path private
  and scoped; do not add public test hooks, restart budgets or change unrelated validation calls.
- [x] **RCD-GUARD-04:** Retain post-outcome expiry checks, so an outcome produced after a
  timely-started poll loses to expiry. Preserve immediate timely incompatibility, compatible
  success, `server_version` update rules and first-poll/zero/short/large-duration behavior.
- [x] **RCD-GUARD-05:** Preserve ReconnectTimeout's requested duration and Unavailable kind,
  retry classification, capped delays and useful already-observed bus diagnostics. Keep a late
  produced error's available context; use truthful pending text when expiry prevents polling.
  Never resume expired work merely to extract an error or classify authorization by text.
- [x] **RCD-GUARD-06:** Preserve unique-owner compatibility, zero ordinary Requests, command
  no-replay, cancellation/state safety, receiver release, QR queues/watchers and all security/
  feature boundaries. Add no detached retries or shared-connection close. Retain the documented
  zbus mid-write limitation without treating the temporary timer-first control as a proven fix.

**Acceptance:** Expired work cannot advance into a new protocol dispatch; timely work and all
earlier deadline/outcome/state invariants remain correct. Document how each guard enforces it.

## RCD-TEST — Cover resumption, stage transitions, and preserved behavior

**Requirements:** TEST-1..3, all DISPATCH and PRESERVE requirements.

- [x] **RCD-TEST-01:** Complete the ready-owner/expired-resumption broker regression. Require
  one resumed poll to yield ReconnectTimeout for the original budget, no new lookup/Version/
  Request, unchanged `server_version`, and connection progress. Count construction and
  subscription negotiations separately; establish zero reconnect Version dispatch before pause.
- [x] **RCD-TEST-02:** Use the same observable owner stage for a timely-resumption control.
  Require exactly one reconnect Version, compatible success with a distinct version and zero
  ordinary Requests. Record phase assumptions and any dependence on pinned zbus ordering.
- [x] **RCD-TEST-03:** Add a narrow internal test for expiry inside an active poll before the
  next lookup/Version stage. Prove the next stage is not activated. Also verify a poll begun in
  time that produces a result after expiry still times out; keep seam behavior tied to production
  dispatch guards rather than testing a duplicate algorithm.
- [x] **RCD-TEST-04:** Cover expiry after a capped retry delay and preserve previous observed
  bus-error context. Retain zero-attempt zero budgets, short absent-service budgets, delayed first
  poll, very large durations, late compatible/incompatible outcomes and late-error diagnostics.
- [x] **RCD-TEST-05:** Retain held-Version timeout, cancellation, late reply, fresh reconnect/
  read and another active subscription on the shared connection. Verify no stale version update,
  lingering retry work, receiver interference or ordinary Request during reconnect.
- [x] **RCD-TEST-06:** Run restart, incompatible replacement, unique-owner and lost-command
  no-replay controls, plus queue saturation/retirement and actual desktop/TUI watcher regressions.
  Preserve the existing watcher path; do not reroute frontends through reconnect for this fix.
- [x] **RCD-TEST-07:** Repeat corrected baseline/final cases at least three times with call
  counts, requested budget, resume/result timestamps and justified tolerances. In an isolated
  checkout/target, restore the missing resume guard/attempt-first behavior and require the new
  no-Version assertion to fail; separately remove post-outcome expiry checking and require the
  existing late-outcome tests to fail. Abort controls when edits do not apply; record exact edits
  and intended failures, never mutate the main checkout for sensitivity experiments.

**Acceptance:** Both dispatch and result semantics have sensitive maintained coverage. New tests
detect the reviewed bug, timely controls pass, negative controls fail for intended reasons and
all prior regressions remain valid. A passing timeout assertion alone is insufficient.

## RCD-VALIDATE — Verify the exact implementation candidate

**Requirements:** EVIDENCE-1, PRESERVE-1..3.

- [ ] **RCD-VALIDATE-01:** Run pinned Rust formatting, locked workspace build/check/tests and
  strict all-target Clippy; run experimental-join daemon tests/Clippy separately. Record per-run
  passed/failed/ignored counts and retain evidence that default production join stays closed.
- [ ] **RCD-VALIDATE-02:** Explicitly run all existing/new private-broker suites and Python
  socket tests. If new ignored cases stay in the existing reconnect suite, verify CI executes
  them without excluding filters; explicitly schedule any new suite. Preserve all owner/queue/
  watcher/authorization/typed-error/no-replay jobs.
- [ ] **RCD-VALIDATE-03:** With pinned Node/npm, run clean frontend install, format, lint,
  typecheck, component/StrictMode tests and production build. Confirm narrow Tauri permissions
  and payload-free notifications remain intact.
- [ ] **RCD-VALIDATE-04:** Build fresh core and desktop Debian packages; inspect manifests,
  dependencies and embedded assets. Run source and extracted-control lifecycle checks plus
  isolated upgrade using current `state_check`, covering identity preservation, v2-to-v3 state
  migration, cleanup failure/retry, remove and purge. Keep helper/systemd mocks and scratch under
  workspace `tmp/`; DPKG_ROOT alone does not isolate D-Bus or host services.
- [ ] **RCD-VALIDATE-05:** Record exact source SHA, environment, commands/results, ignored cases,
  broker timings/counts, control edits, fresh package hashes and limitations in versioned evidence.
  Read exact-candidate CI through Ralph Bridge. Attribute earlier CI/results to their own
  revisions; do not reuse historical hashes or sum overlapping test runs as a new total.

**Command reference:** repository root, Rust 1.95.0:

```bash
cargo fmt --all -- --check
cargo build --workspace --locked
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p blueroute-daemon --features experimental-join --locked
cargo clippy -p blueroute-daemon --all-targets --features experimental-join --locked -- -D warnings
dbus-run-session -- cargo test -p blueroute-client --test reconnect_deadline --locked -- --ignored
python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v
bash packaging/debian/test-maintainer-lifecycle.sh
bash packaging/debian/build-package.sh
cargo build --release --locked -p blueroute-linux --example state_check
```

Run every remaining broker suite in [ci.yml](../.github/workflows/ci.yml) using its documented
package, suite and filters. Default workspace execution leaves ignored tests unexecuted.

In `apps/desktop`, use Node 26.13.0/npm 11.20.0: `npm ci`, `npm run format:check`, `npm run lint`,
`npm run typecheck`, `npm test`, `npm run build`. Put the pinned Node directory on PATH for
package builds. Run `bash packaging/debian/test-package-upgrade.sh <fresh-core-package.deb>
target/release/examples/state_check`; extract fresh control scripts under workspace `tmp/` and
set `BLUEROUTE_MAINTAINER_SCRIPT_DIR` there for packaged-script lifecycle verification.

**Acceptance:** Required software/package checks and exact-candidate CI pass; new regressions
actually execute. Counts and artifacts identify the candidate, mocked layers and remaining limits.

## RCD-CLOSEOUT — Reconcile the contract and acceptance claims

**Requirements:** EVIDENCE-2/3 and every requirement-to-evidence mapping.

- [ ] **RCD-CLOSEOUT-01:** Add dated RRC-01 correction/authority links to the previous reconnect
  backlog/evidence. Reconcile RC-DEADLINE-03, RC-TEST-02 and RC-CLOSEOUT-01/03/04, their parent
  states and acceptance assertions with the review. Preserve valid fixes/historical runs; an
  earlier all-green validation matrix must not be treated as proof of no post-expiry dispatch.
- [ ] **RCD-CLOSEOUT-02:** Review the whole reconnect poll/stage path, dispatch decision points,
  ready-owner regression, in-poll expiry, both sensitivity controls, diagnostics, state changes,
  owner/no-replay safety and cancellation limits. Resolve review findings before acceptance.
- [ ] **RCD-CLOSEOUT-03:** Align public rustdoc, product specification and dated P5-005 client/
  backlog guidance with dispatch and outcome semantics. Preserve prior RC/QR/LC/FU/CR history
  and link this backlog as RRC-01 authority. Do not claim to fix arbitrary zbus mid-write safety.
- [ ] **RCD-CLOSEOUT-04:** Create versioned implementation evidence mapping all 14 companion
  requirements to tasks, source, regressions, baseline/control failures, final results and limits.
  Verify document links, exact revision identity, parent/leaf states and consistent acceptance
  rows. Reconcile earlier DEADLINE-3, EVIDENCE-1/3 mappings using the new evidence.
- [ ] **RCD-CLOSEOUT-05:** Close this scoped software correction only after all acceptance
  criteria pass. Report existing installed-system/physical/product/release gates separately and
  preserve their open status. Record no hardware-support claim or new equipment dependency.

**Acceptance:** RRC-01 and its coverage/documentation gaps have a reviewed, reproducible software
resolution; current states and evidence agree. Creation of a plan or success of a temporary
timer-first experiment establishes no implementation acceptance.

## Requirement coverage and evidence ledger

Populate versioned evidence during implementation; no fix SHA or result is established yet.

| Requirements | Tasks | Required evidence | State |
| --- | --- | --- | --- |
| DISPATCH-1 | RCD-GUARD-01/04/05, RCD-TEST-04 | Single budget, first-poll/zero/short/large-duration regressions | PENDING |
| DISPATCH-2 | RCD-GUARD-02, RCD-TEST-01 | Ready-owner expired resumption; no extra dispatch on resumed poll | PENDING |
| DISPATCH-3 | RCD-GUARD-03, RCD-TEST-03 | Explicit dispatch decision guards; in-poll stage activation test | PENDING |
| DISPATCH-4 | RCD-GUARD-04, RCD-TEST-03/04/07 | Late outcome precedence and post-outcome negative control | PENDING |
| DISPATCH-5 | RCD-GUARD-05, RCD-TEST-04 | Typed timeout/requested budget and truthful available diagnostics | PENDING |
| PRESERVE-1 | RCD-GUARD-06, RCD-TEST-06 | Owner/compatibility/zero Request/no-replay regressions | PENDING |
| PRESERVE-2 | RCD-GUARD-06, RCD-TEST-05 | State, cancellation, fresh use, active subscription and transport limits | PENDING |
| PRESERVE-3 | RCD-GUARD-06, RCD-TEST-06, RCD-VALIDATE | QR/feature/security/frontend/package invariants | PENDING |
| TEST-1 | RCD-BASE-03/04, RCD-TEST-01 | Maintained production-path call-count regression, not temporary-only probe | PENDING |
| TEST-2 | RCD-BASE-04, RCD-TEST-02..06 | Complete boundary matrix and isolated, bounded fixtures | PENDING |
| TEST-3 | RCD-BASE-03, RCD-TEST-07 | Baseline/final repeats and sensitive isolated negative controls | PENDING |
| EVIDENCE-1 | RCD-VALIDATE-01..05 | Exact-candidate validation, package hashes and scheduled CI | PENDING |
| EVIDENCE-2 | RCD-CLOSEOUT-01/03/04 | Dated corrections, public contract and complete requirement map | PENDING |
| EVIDENCE-3 | RCD-CLOSEOUT-02/05 | Reviewed software-only closure; other qualification stays separate | PENDING |

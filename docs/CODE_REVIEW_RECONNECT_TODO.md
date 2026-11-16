# Code Review Reconnect Deadline TODO

**Status:** COMPLETE (2026-11-16, candidate `a7abc60`, CI 43940470468); software-only acceptance. Evidence:
[CODE_REVIEW_RECONNECT_EVIDENCE.md](CODE_REVIEW_RECONNECT_EVIDENCE.md).  
**Finding / priority:** RQR-01 / P2.  
**Baseline:** `bd58ce2355816d7c7b956b9ed7dceedb4925044a`.  
**Companion specification:** [CODE_REVIEW_RECONNECT_SPEC.md](CODE_REVIEW_RECONNECT_SPEC.md).  
**Related work:** [original backlog](TODO.md), [queue/recovery backlog](CODE_REVIEW_QUEUE_RECOVERY_TODO.md),
and [queue/recovery evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md).

## Tracking and scope

- Use stable `RC-*` IDs with RQR-01 and original P5-005 references. The register below is the
  current status authority for this correction; keep parents, leaves and evidence consistent.
- `[ ]` pending, `[-]` in progress, `[x]` accepted, `[!]` blocked with a reason. Creating this
  plan completes no implementation or acceptance. Check items only after their criteria pass.
- Create versioned implementation evidence and link it from the ledger: source SHA, test paths,
  commands/results, timing/tolerance, negative controls, package hashes, CI and limitations.
  Temporary review artifacts must not be the only reproduction or acceptance record.
- This is a reusable-client deadline fix. Preserve existing QR work and all security/compatibility
  boundaries; no daemon/frontend feature, role-editing API or storage/wire change is required.
- Acceptance is software-only. Preserve existing installed-system/physical and product/release
  gates as separate work, with no support claim or new equipment dependency for this fix.

## Task register and dependencies

| Task | Outcome | Dependencies | State |
| --- | --- | --- | --- |
| RC-BASE | Durable reproduction and corrected failing expectations | None | `[x]` |
| RC-DEADLINE | One deadline across lookup, Version and backoff | RC-BASE | `[x]` |
| RC-TEST | Timing boundaries, cancellation and compatibility regressions | RC-DEADLINE design; tests may precede implementation | `[x]` |
| RC-VALIDATE | Exact-candidate checks, packages and CI scheduling | RC-DEADLINE, RC-TEST | `[x]` |
| RC-CLOSEOUT | Public contract, P5-005 reconciliation and reviewed software closure | RC-DEADLINE, RC-TEST, RC-VALIDATE | `[x]` |

Tests and public-contract updates can start with the deadline design. Keep implementation local
to the shared client; continue independent work without waiting for existing hardware gates.

## RC-BASE — Establish the deadline failure and isolated fixtures

**Requirements:** EVIDENCE-1/3; RQR-01 reproduction in the specification.

- [x] **RC-BASE-01:** Record HEAD, dirty state, toolchains and differences from the reviewed
  baseline. Read applicable guidance and current product/client/remediation requirements.
- [x] **RC-BASE-02:** Preserve the held-Version 50 ms/250 ms reproduction, late-success outcome,
  affected code and P5-005 relationship in versioned evidence. Record that this predates QR fixes
  and is distinct from the shared watcher's bounded backoff.
- [x] **RC-BASE-03:** Add a corrected expectation using the actual public reconnect method:
  timeout must be returned while Version remains held. Demonstrate failure at the baseline;
  retain a timely-response control.
- [x] **RC-BASE-04:** Use private session buses, finite scripted daemons, release/drop guards,
  bounded watchdogs and ignored workspace `tmp/` artifacts. Prevent fixture failure/cancellation
  from hanging teardown or reaching host services.

**Acceptance:** A maintained corrected regression fails for the reported unbounded negotiation,
with controlled setup, requested budget and measured outcome recorded.

## RC-DEADLINE — Enforce a single whole-operation budget

**Requirements:** DEADLINE-1..6, COMPAT-1..3.

- [x] **RC-DEADLINE-01:** Define budget start, monotonic deadline, expiry-versus-ready-reply
  precedence, zero-duration behavior, typed diagnostics and informational-version update rules.
- [x] **RC-DEADLINE-02:** Bound the full current-owner/Version negotiation, including internal
  setup awaits, by the remaining operation budget. Do not wait for a longer transport timeout
  or close the shared connection to terminate a read-only attempt.
- [x] **RC-DEADLINE-03:** Cap retry sleep to remaining time and stop before dispatching another
  lookup/Version after expiry. Preserve existing retryable-error and early incompatibility rules.
- [x] **RC-DEADLINE-04:** Return the existing ReconnectTimeout with the requested duration and
  useful existing error context or a truthful pending-negotiation timeout description. Preserve
  Unavailable classification; never fabricate a bus failure or classify authorization by text.
- [x] **RC-DEADLINE-05:** Make zero/short budgets obey the contract without an initial attempt
  for zero, a full 100 ms sleep for a shorter budget, or late success when both results are ready.
- [x] **RC-DEADLINE-06:** Update `server_version` only on timely compatible success; expiry,
  cancellation and late replies must not mutate it. Release operation-owned work and retain
  connection usability for later operations and active subscriptions.
- [x] **RC-DEADLINE-07:** Preserve owner-bound compatibility negotiation, zero ordinary Requests
  during reconnect, mutation no-replay, QR subscription behavior, watcher backoff, authorization,
  API/schema and closed join/sharing boundaries. Add no new background retry/monitor tasks.

**Acceptance:** Lookup, Version and backoff share one deadline. Expiry wins at the defined outcome
decision, produces the existing typed timeout, and preserves client state/connection safety.

## RC-TEST — Cover timing boundaries and preserved behavior

**Requirements:** EVIDENCE-1, DEADLINE-1..6, COMPAT-1/2/3.

- [x] **RC-TEST-01:** On a private broker, hold and never reply to reconnect's Version request;
  require ReconnectTimeout before releasing it. Record budget, measured latency and justified
  tolerance, with a watchdog long enough to expose the original failure.
- [x] **RC-TEST-02:** Cover delayed owner lookup so the timeout cannot accidentally apply only
  to Version. Use a narrow internal timing seam when necessary, retaining the production-path
  broker regression and keeping test hooks out of the public API.
- [x] **RC-TEST-03:** Test zero duration and service absence with a budget shorter than the
  poll interval. Count attempts; require no zero-budget negotiation or post-expiry retry.
- [x] **RC-TEST-04:** Make reply and timer both ready after expiry, for compatible and incompatible
  replies. Require timeout and unchanged informational version; use a distinct compatible server
  version so a late update would be observable.
- [x] **RC-TEST-05:** Verify timely compatible success, immediate timely incompatibility and
  normal restart recovery. Preserve unique-owner binding, zero ordinary Requests to incompatible
  successors and no replay after possible command delivery.
- [x] **RC-TEST-06:** Cancel an in-flight reconnect, release a late reply, then perform a fresh
  reconnect/read-only call and exercise another active subscription on the same connection.
  Verify unchanged cancelled/expired state, resource cleanup and preserved QR queue regressions.

**Acceptance:** Maintained tests cover the full timing/outcome matrix, detect the original defect,
and distinguish the client's timeout from fixture watchdog expiry. Perform a safe isolated
negative control restoring retry-only deadline checks and record the intended failures.

## RC-VALIDATE — Validate the exact candidate and schedule regressions

**Requirements:** EVIDENCE-2, COMPAT-1..3.

- [x] **RC-VALIDATE-01:** Run pinned Rust formatting, locked workspace build/check/tests and
  strict all-target Clippy, plus experimental-join software tests. Record counts/ignored cases
  and verify default builds keep join closed.
- [x] **RC-VALIDATE-02:** Run all existing/new private-broker suites and Python socket-tooling
  checks. Explicitly schedule every new ignored deadline suite in CI; retain queue saturation,
  owner binding/watchers, error classification and no-replay coverage.
- [x] **RC-VALIDATE-03:** With pinned Node/npm, run clean frontend install, format/lint/type
  checks, component/StrictMode tests and production build; retain narrow Tauri permissions.
- [x] **RC-VALIDATE-04:** Build both Debian packages and inspect manifests/dependencies/assets.
  Run source and extracted packaged-script lifecycle, state migration and isolated upgrade /
  cleanup failure/retry/remove/purge checks against the fresh candidate.
- [x] **RC-VALIDATE-05:** Record exact source SHA, toolchains, commands/results, package hashes,
  preserved-invariant evidence and limitations; inspect exact-revision CI with repository-approved
  tooling. Attribute earlier runs to their own revisions and do not sum overlapping test suites.

**Command reference:** from the repository root, with Rust 1.95.0:

```bash
cargo fmt --all -- --check
cargo build --workspace --locked
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p blueroute-daemon --features experimental-join --locked
dbus-run-session -- cargo test -p blueroute-client --test dbus_client --locked -- --ignored
dbus-run-session -- cargo test -p blueroute-client --test owner_race --locked -- --ignored
dbus-run-session -- cargo test -p blueroute-client --test queue_saturation --locked -- --ignored
python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v
bash packaging/debian/test-maintainer-lifecycle.sh
bash packaging/debian/build-package.sh
cargo build --release --locked -p blueroute-linux --example state_check
```

Run the remaining existing broker suites in [ci.yml](../.github/workflows/ci.yml) and every new
deadline suite, using `dbus-run-session -- cargo test -p <package> --test <suite> --locked --
--ignored` for private-broker files. A default workspace test does not execute ignored cases.

In `apps/desktop`, use Node 26.13.0/npm 11.20.0: `npm ci`, `npm run format:check`, `npm run lint`,
`npm run typecheck`, `npm test` and `npm run build`.

Run `bash packaging/debian/test-package-upgrade.sh <fresh-core-package.deb>
target/release/examples/state_check`. Extract fresh package control scripts into an ignored
temporary directory and point `BLUEROUTE_MAINTAINER_SCRIPT_DIR` there when running
`test-maintainer-lifecycle.sh`. Keep package-test scratch/logs under workspace `tmp/`; retain
helper/systemd mocks because `DPKG_ROOT` relocates files and does not isolate D-Bus.

**Acceptance:** Required software/package checks and exact-candidate CI pass; new deadline tests
actually run and evidence identifies mocked layers, ignored cases and remaining limitations.

## RC-CLOSEOUT — Review the fix and reconcile the bounded-reconnect claim

**Requirements:** EVIDENCE-3 and every requirement-to-evidence mapping.

- [x] **RC-CLOSEOUT-01:** Review the full deadline path and timing matrix, negative control,
  cancellation/late-reply behavior, typed errors, client-state updates and owner/no-replay safety.
- [x] **RC-CLOSEOUT-02:** Document public timeout semantics and update relevant specification /
  client guidance. Append a dated P5-005 correction/evidence link for bounded reconnect, preserving
  its working request/event/version-gating behavior and earlier QR/LC/FU/CR history.
- [x] **RC-CLOSEOUT-03:** Complete versioned evidence mapping all 12 requirements to tasks,
  source, corrected baseline failures, final results and limits; verify links, candidate identity
  and states. Link this backlog as the current RQR-01 authority.
- [x] **RC-CLOSEOUT-04:** Close this software deadline remediation only after implementation,
  tests, validation, documentation and review pass. Report existing installed-system/physical /
  product/release gates separately and preserve their open status without adding an unrelated
  equipment dependency to this fix or making a hardware-support claim.

**Acceptance:** RQR-01's caller-supplied timeout contract is evidenced and documented. This scoped
software outcome is distinct from earlier queue/recovery acceptance and remaining qualification.

## Evidence ledger

Populate as work is accepted; create the versioned evidence record during implementation.
Preserve exact revisions and separate new results from historical acceptance.

| Task / finding | Fix SHA / source | Regression, control and validation evidence | Remaining acceptance | State |
| --- | --- | --- | --- | --- |
| RC-BASE / RQR-01 | `bd58ce2` (baseline) + `tests/reconnect_deadline.rs` | 5 corrected expectations fail at the baseline on 3 repeats; 5 controls pass ([evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md#rc-base--finding-preserved-and-corrected-expectations-established-2026-11-16)) | None | ACCEPTED |
| RC-DEADLINE / RQR-01 | `3e05d98`, `crates/blueroute-client/src/lib.rs` (`negotiate_within`) | DEADLINE-1..6, COMPAT-1..3 mapped ([evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md#rc-deadline--one-whole-operation-budget-2026-11-16)); existing client suites pass | None | ACCEPTED |
| RC-TEST | `tests/reconnect_deadline.rs` (10), `deadline_tests` (6) | Full matrix passes 3/3; controls X1–X4 each caught ([evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md#rc-test--timing-outcome-and-cancellation-matrix-2026-11-16)) | None | ACCEPTED |
| RC-VALIDATE | `a7abc60` | All Rust/broker/Python/frontend/package checks pass; CI 43940470468 success ([evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md#rc-validate--exact-candidate-2026-11-16)) | None | ACCEPTED |
| RC-CLOSEOUT | `af58349` (review fixes), `a7abc60` | Independent review resolved; 12 requirements mapped ([evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md#rc-closeout--requirement-map-and-closure-2026-11-16)) | None for RQR-01; existing QR/LC/FU/CR gates stay open separately | ACCEPTED |

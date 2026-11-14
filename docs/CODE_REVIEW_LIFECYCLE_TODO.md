# Code Review Lifecycle Remediation TODO

**Status:** IN PROGRESS (2026-11-14); see the register and evidence ledger.  
**Baseline:** `75c1a01746557bb13f525a55642b497e3bd5f81d`.  
**Companion specification:** [CODE_REVIEW_LIFECYCLE_SPEC.md](CODE_REVIEW_LIFECYCLE_SPEC.md).  
**Earlier work:** [follow-up backlog](CODE_REVIEW_FOLLOWUP_TODO.md),
[follow-up evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md), and [original backlog](TODO.md).

## Tracking rules

- Preserve stable `LC-*` IDs alongside RFU-01/02 and existing FU/CR/P references. The register
  below is the current authority for this work; keep parent states, leaves and evidence consistent.
- Use `[ ]` pending, `[-]` in progress, `[x]` accepted and `[!]` blocked with a reason.
  Every checkbox starts pending; creating this plan establishes no implementation or acceptance.
- Accept a task only after its behavior, required regressions and parent acceptance pass. Record
  exact source SHA, commands/results, negative controls and limitations in versioned evidence
  linked from the ledger below. Do not rely on temporary review artifacts.
- Continue software work while equipment is deferred. Keep required installed-system/physical
  gates open and distinguish SOFTWARE COMPLETE from full cleanup/release qualification.
- Preserve pinned toolchains, contributor guidance and the specification's security, protocol,
  schema, ownership, join, sharing and frontend boundaries. Scope fixes to the two findings.

## Maintainer decisions (2026-11-14)

- **CI evidence:** exact-revision CI is inspected with git and the GitHub CLI (`gh run`),
  recording run IDs and head SHAs. Ralph Bridge is not used for this backlog.
- **Unknown-role cleanup policy:** follow the specification as written. Member/None and a client
  record without a membership row stay unresolved and fail removal/purge with proof retained;
  no production role-editing API is added. Document a manual administrator recovery in
  `docs/ADMINISTRATOR.md` (confirm no PANU link remains, then remove the stale client record or
  the network's owned NetworkManager profile by hand) alongside the evidence-based recovery.

## Task register and dependencies

| Task | Outcome | Dependencies | State |
| --- | --- | --- | --- |
| LC-BASE | Durable reproductions and isolated failing tests | None | `[-]` |
| LC-OWNER | P2 RFU-01: ownership-period retirement | LC-BASE | `[x]` |
| LC-CLEANUP | P2 RFU-02: unknown active role and proof retention | LC-BASE | `[ ]` |
| LC-INTEGRATION | Actual consumer/helper boundaries and CI coverage | LC-OWNER, LC-CLEANUP | `[ ]` |
| LC-VALIDATE | Exact-candidate software/package/CI checks | LC-INTEGRATION | `[ ]` |
| LC-QUALIFY | Existing installed-system/physical evidence gates | Procedure: LC-CLEANUP design; execution: validated packages and designated equipment | `[ ]` |
| LC-CLOSEOUT | Documentation reconciliation and reviewed closure | Software: LC-OWNER/CLEANUP/INTEGRATION/VALIDATE; full: LC-QUALIFY | `[ ]` |

LC-OWNER and LC-CLEANUP can proceed independently after baseline preparation. Documentation and
procedure corrections can start with their designs. Equipment is not a dependency for the two
software fixes or their deterministic/private-broker regressions.

## LC-BASE — Preserve findings and establish corrected expectations

**Requirements:** VERIFY-1/2, CLOSE-3; RFU-01/02 recipes in the specification.

- [x] **LC-BASE-01:** Record HEAD, dirty state, toolchains and differences from the reviewed
  baseline. Read applicable guidance and current product/follow-up/lifecycle requirements.
- [x] **LC-BASE-02:** Preserve RFU-01/02 triggers, affected paths, observed outcomes and earlier
  task relationships in versioned evidence, including the limited scope of each reproduction.
- [x] **LC-BASE-03:** Add a corrected ownership-period test that fails when a ready A→B→A
  Version reply delivers the old event before queued OwnerLost. Control the order with barriers.
- [-] **LC-BASE-04:** Add a production-store Member/None plus matching client-record test that
  fails when cleanup without Bluetooth deletes proof and reports success. Preserve valid state.
- [x] **LC-BASE-05:** Verify fixtures use private buses, fake networking and temporary state /
  package roots. Mock the installed helper where necessary so package tests cannot reach host buses.

**Acceptance:** Both defects have maintained corrected expectations failing on the baseline for
the intended reason; setup and teardown are isolated and bounded.

## LC-OWNER — Retire a subscription when its ownership period ends

**Requirements:** EPOCH-1..5. **Earlier tasks:** FU-02-02/03, FU-INTEGRATION-02.

- [x] **LC-OWNER-01:** Define subscription ownership periods, delivery decision ordering,
  queued owner notices, transient verification errors, cancellation and monitor resource lifetime.
- [x] **LC-OWNER-02:** Account for retirement at signal selection, during Version and during
  final verification. Prevent a ready reply or reacquired unique name from bypassing a transition.
- [x] **LC-OWNER-03:** Keep retirement terminal, including late replies/queued events; ensure
  cancellation cannot consume a retirement without recording it. Require a fresh subscription.
- [x] **LC-OWNER-04:** Test A→B→A with the consumer paused until reply and notices are ready,
  for typed Event and StateChanged. Require OwnerLost first and rejection on every later old wait.
- [x] **LC-OWNER-05:** Cover A→unowned→A, retirement before signal selection, held/late Version,
  final-verification ordering and cancellation during validation; use controlled acknowledgements.
- [x] **LC-OWNER-06:** Verify fresh returning-A subscription and snapshot refresh; retain ordinary
  compatible/incompatible B, no successor, healthy A, and transient-check-error behavior. Preserve
  the two-second actively-polled loss bound, bounded backoff and no busy polling/task leaks.
- [x] **LC-OWNER-07:** Recheck unique-owner Version/Request destinations, compatibility rules,
  zero ordinary Requests to incompatible successors, and no mutation replay after possible delivery.

**Acceptance:** RFU-01 fails at the baseline and passes with all required ordering scenarios.
Current-name equality cannot revive an old subscription; healthy delivery and command safety work.

## LC-CLEANUP — Keep ambiguous active membership unresolved

**Requirements:** SCOPE-1..6. **Earlier tasks:** FU-04-01/02/03/06/07/09, FU-INTEGRATION-03.

- [ ] **LC-CLEANUP-01:** Define cleanup classification for every specification table row,
  preserving membership state, role uncertainty and record provenance instead of a host/client
  boolean alone. Update scope/report callers and fixtures; document how an orphan client record
  differs from a confirmed completed leave.
- [ ] **LC-CLEANUP-02:** Keep Member/None unresolved with or without a client hint, with either
  Bluetooth availability. Preserve dependent profile/address proof, membership and client records.
- [ ] **LC-CLEANUP-03:** Include ambiguity in the actual cleanup report/helper failure outcome,
  even when remaining-resource inventory alone is zero; use bounded non-secret diagnostics.
- [ ] **LC-CLEANUP-04:** Continue safe work for other confirmed networks without touching foreign
  resources. Preserve instance locking, removal-marker quiescence, run bounds and typed adapters.
- [ ] **LC-CLEANUP-05:** Test Member/None through production stores and scope construction:
  Bluetooth absent/present, hint absent/present, owned/foreign fixtures, no remaining profile,
  conflicting proof and unrelated-network progress. Assert records as well as resource counts.
- [ ] **LC-CLEANUP-06:** Retain explicit Host, explicit Client, NotMember/stale-record and invalid
  storage behavior. Test missing membership rows with client records; no hint alone authorizes
  disconnect and no unknown role is silently rewritten into a confirmed role.
- [ ] **LC-CLEANUP-07:** Document evidence-based recovery; test authoritative recovered state,
  reobservation, interruption/retry and repeat success. Keep existing proof retention and schema;
  do not add a production role-editing command or infer a role from Bluetooth availability.
- [ ] **LC-CLEANUP-08:** Propagate unresolved scope through actual helper outcome and packaged
  remove/deconfigure/direct-purge failure/retry paths. Retain evidence on failure and preserve
  upgrade/abort behavior and successful confirmed completed-leave cleanup.

**Acceptance:** RFU-02 cannot succeed by treating an unknown active role as host-only. The proof
survives unresolved cleanup; independent safe work, valid recovery and prior stale-record fixes pass.

## LC-INTEGRATION — Exercise actual boundaries and sensitivity

**Requirements:** VERIFY-1/2/4, EPOCH-4, SCOPE-6.

- [ ] **LC-INTEGRATION-01:** Carry ownership-period loss through the production shared watcher,
  desktop and TUI adapters. Require disconnected/stale before fresh connection/change; reject old
  payloads and validate snapshots on returning A as well as ordinary B.
- [ ] **LC-INTEGRATION-02:** Exercise production store→scope→cleanup→report→helper outcome for
  unknown roles, then maintainer/isolated-dpkg removal failure, preserved state, recovery and purge.
  Identify mocked layers explicitly; retain safe adapter UnknownObject failure/proof preservation.
- [ ] **LC-INTEGRATION-03:** Add new tests to CI, explicitly scheduling required ignored broker
  suites on private buses. Verify no required regression is skipped by default/filter changes.
- [ ] **LC-INTEGRATION-04:** Restore each defect safely in a temporary checkout or equivalent
  isolated setup; confirm its corrected regression fails for the intended reason. Record results.
- [ ] **LC-INTEGRATION-05:** Re-run existing publisher progress/shutdown, startup handoff, graph
  import/atomicity, ordinary owner handover, client cleanup/retry, NotMember stale-record, UI
  effect-generation and security regressions. Resolve new failures before acceptance.

**Acceptance:** Both fixes work through real production boundaries and have sensitive maintained
regressions. Passing a mock helper or a direct scope constructor alone is insufficient evidence.

## LC-VALIDATE — Check the exact software candidate

**Requirements:** VERIFY-3/4; preserved platform and security invariants.

- [ ] **LC-VALIDATE-01:** Run pinned Rust formatting, locked workspace build/check/tests and
  strict all-target Clippy; record actual passed/failed/ignored counts and source revision.
- [ ] **LC-VALIDATE-02:** Run all existing/new private-broker suites, experimental-join software
  tests and Python socket tooling. Verify default package builds retain the closed join gate.
- [ ] **LC-VALIDATE-03:** With pinned Node/npm, run clean frontend install, format/lint/type
  checks, component/StrictMode tests and production build; retain narrow Tauri permissions.
- [ ] **LC-VALIDATE-04:** Build both Debian packages; inspect manifests, dependencies and embedded
  UI. Run source/packaged lifecycle tests, state migration, isolated upgrade and remove/purge/retry.
- [ ] **LC-VALIDATE-05:** Verify authorization, authenticated identities/pins/revocation, rollback,
  foreign preservation, stable identity/schema, no replay and disabled sharing remain intact.
- [ ] **LC-VALIDATE-06:** Record candidate SHA, toolchains, commands/results, package hashes and
  limitations. Check exact-revision CI with `gh run`; attribute historical runs to their SHAs.

**Command reference:** run Rust/Python/package commands at the repository root.

```bash
cargo fmt --all -- --check
cargo build --workspace --locked
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p blueroute-daemon --features experimental-join --locked
dbus-run-session -- cargo test -p blueroute-client --test event_owner_race --locked -- --ignored
dbus-run-session -- cargo test -p blueroute-client --test watcher --locked -- --ignored
cargo test -p blueroute-daemon --lib --locked removal
python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v
bash packaging/debian/test-maintainer-lifecycle.sh
bash packaging/debian/build-package.sh
cargo build --release --locked -p blueroute-linux --example state_check
```

Run remaining existing broker suites from [ci.yml](../.github/workflows/ci.yml), plus new suites
added by LC-INTEGRATION. Use `dbus-run-session -- cargo test -p <package> --test <suite> --locked
-- --ignored` for private-broker files. A successful workspace test run alone leaves ignored cases
unexecuted.

In `apps/desktop`, run `npm ci`, `npm run format:check`, `npm run lint`, `npm run typecheck`,
`npm test` and `npm run build` with Node 26.13.0/npm 11.20.0. Use Rust 1.95.0 for Rust checks.

Run `bash packaging/debian/test-package-upgrade.sh <fresh-core-package.deb>
target/release/examples/state_check` with the exact freshly built core package. Extract its
control scripts to a temporary directory and run `test-maintainer-lifecycle.sh` with
`BLUEROUTE_MAINTAINER_SCRIPT_DIR` pointing there for packaged-script validation. These isolated
harnesses do not require installing/removing packages or reaching real networking on the host.

**Acceptance:** All required software checks and exact-candidate CI pass; counts, ignored tests,
scope and limitations are recorded without installed-system/physical claims.

## LC-QUALIFY — Preserve the existing equipment-dependent gates

**Requirements:** CLOSE-2/3. **Existing criteria:** FU-QUALIFY and CR-QUALIFY-01..08;
[qualification procedure](CR-QUALIFY-PROCEDURE.md),
[hardware tracker](HARDWARE_ACCEPTANCE_BACKLOG.md).

- [ ] **LC-QUALIFY-01:** Extend adapter-free Part A fixtures/procedure with active unknown role
  plus client record, retained proof and failed removal/purge; retain confirmed completed-leave
  and recovery cases. State clearly that procedure readiness is separate from execution.
- [ ] **LC-QUALIFY-02:** On designated disposable equipment, execute the revised installed-system
  procedure and record actual NetworkManager/kernel/systemd/dpkg outcomes and foreign comparisons.
- [ ] **LC-QUALIFY-03:** For physical PAN claims, execute existing Part B on the designated rig;
  record machines/controllers/software, candidate/package identity and independent observations.
  Link evidence to existing FU/CR qualification and hardware gates rather than duplicating closure.
- [ ] **LC-QUALIFY-04:** If equipment remains unavailable, record the deferral and keep unmet
  installed-system/physical criteria and this parent open. Continue independent software work.

**Acceptance:** Existing required environment criteria have actual evidence. Recording deferral
can complete its reporting leaf, but cannot complete this parent or establish physical support.

## LC-CLOSEOUT — Reconcile status and review closure

**Requirements:** CLOSE-1..3 and every requirement's evidence mapping.

- [ ] **LC-CLOSEOUT-01:** Review RFU-01/02 fixes against baseline recipes, all EPOCH/SCOPE
  criteria, additional adverse cases, negative controls and preserved security/ownership boundaries.
- [ ] **LC-CLOSEOUT-02:** Append dated corrections to FU-02/FU-04/FU-INTEGRATION/FU-CLOSEOUT
  claims and the follow-up evidence. Preserve earlier successful behavior, historical results,
  and original task IDs; link this backlog as the current RFU-01/02 authority.
- [ ] **LC-CLOSEOUT-03:** Update affected product/current summaries, client lifecycle contract,
  cleanup classification/design and qualification procedure. Preserve unsatisfied original gates.
- [ ] **LC-CLOSEOUT-04:** Complete versioned evidence and map all 18 specification requirements
  to tasks, source, regressions or qualification records. Verify links, source SHAs and task states.
- [ ] **LC-CLOSEOUT-05:** Record software closure only after both defects, integration, validation
  and documentation pass. Keep full closure open until LC-QUALIFY and existing required gates pass;
  use SOFTWARE COMPLETE / QUALIFICATION DEFERRED when appropriate.

**Acceptance:** Every finding and applicable criterion has an evidenced outcome. Current status
reflects actual software and qualification results without erasing historical acceptance records.

## Evidence ledger

Populate this ledger as work is accepted; create the versioned evidence record during implementation.
Keep software outcomes separate from actual installed-system/physical results.

| Task / finding | Fix SHA and source | Regression / negative control / software evidence | Qualification or remaining gate | State |
| --- | --- | --- | --- | --- |
| LC-BASE | `e167198` baseline | [Evidence](CODE_REVIEW_LIFECYCLE_EVIDENCE.md#lc-base--findings-preserved-and-corrected-expectations-established-2026-11-14): RFU-01 2/2 corrected tests fail; RFU-02 test fails, committed with its fix | Isolated reproducible fixtures | IN PROGRESS |
| LC-OWNER / RFU-01 | LC-OWNER commit, `crates/blueroute-client/src/lib.rs` | [Evidence](CODE_REVIEW_LIFECYCLE_EVIDENCE.md#lc-owner--subscriptions-retire-when-their-ownership-period-ends-2026-11-14): EPOCH-1..5, 9 broker tests and 1 unit test; 5/9 fail on both negative controls | Software ownership-period acceptance | ACCEPTED (software) |
| LC-CLEANUP / RFU-02 | — | SCOPE-1..6 pending | LC-QUALIFY for actual installed/PAN claims | NOT STARTED |
| LC-INTEGRATION | — | VERIFY-1/2/4 pending | Actual consumer/helper boundaries and CI scheduling | NOT STARTED |
| LC-VALIDATE | — | VERIFY-3/4 pending | Exact-candidate checks/packages/CI | NOT STARTED |
| LC-QUALIFY | — | Procedure revision pending | FU-QUALIFY, CR-QUALIFY-01..08, hardware tracker | NOT STARTED |
| LC-CLOSEOUT | — | CLOSE-1..3 pending | Software milestone; full qualification separately | NOT STARTED |

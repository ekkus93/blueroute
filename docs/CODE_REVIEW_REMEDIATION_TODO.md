# Code Review Remediation TODO

**Status:** IN PROGRESS. **Baseline:** `4ca5042ee28743dd30782379fbcf8fa1ed15df77`.  
**Companion specification:** [CODE_REVIEW_REMEDIATION_SPEC.md](CODE_REVIEW_REMEDIATION_SPEC.md).  
**Original implementation backlog:** [TODO.md](TODO.md). **Evidence:** [CODE_REVIEW_REMEDIATION_EVIDENCE.md](CODE_REVIEW_REMEDIATION_EVIDENCE.md).

## Tracking rules

- `CR-*` IDs are stable remediation IDs; preserve original P0–P19 IDs and cross-reference them.
- Every checkbox below starts unchecked. Creating these documents does not complete a fix.
- `[ ]` means pending, `[-]` in progress, `[x]` accepted, and `[!]` blocked with an explanation.
- Check a task only after its implementation and acceptance evidence pass. Record exact source
  SHA, commands/results, related findings, and remaining limitations in its evidence entry.
- Software-only completion cannot close a task with required installed-system or physical
  acceptance. Use **SOFTWARE COMPLETE / HARDWARE DEFERRED** and retain its open gate.
- Original incomplete features remain outside this work unless necessary to fix one of F1–F8.
  Do not open production join, enable Internet sharing, or turn the read-only TUI into a mutator.

## Task register and dependencies

| Task | Finding / outcome | Priority | Dependencies | State |
| --- | --- | --- | --- | --- |
| CR-BASE | Durable baseline and reproductions | Foundation | None | `[-]` |
| CR-F8 | Owner-bound compatibility and command delivery | P2 | CR-BASE | `[x]` |
| CR-F2 | Idle owner-loss detection and watcher recovery | P2 | CR-BASE; owner contract from CR-F8 | `[x]` |
| CR-F1 | Committed state publication and observer refresh | P2 | CR-BASE; CR-F2 for integrated acceptance | `[x]` |
| CR-F3 | Consistent graph insertion/update semantics | P2 | CR-BASE | `[ ]` |
| CR-F4 | Fresh operational capabilities after recovery | P2 | CR-BASE; CR-F1 for observer acceptance | `[ ]` |
| CR-F5 | Ownership-safe remove/purge cleanup | P2 | CR-BASE; lifecycle design | `[ ]` |
| CR-F6 | Effect-generation resource ownership | P3 | CR-BASE; watcher contract from CR-F2 | `[ ]` |
| CR-F7 | Current documentation and acceptance reconciliation | P3 | Baseline fixes can start immediately; final pass follows implementation | `[ ]` |
| CR-INTEGRATION | Real producer-to-consumer regression coverage | Acceptance | CR-F1/F2/F3/F4/F5/F6/F8 software work | `[ ]` |
| CR-VALIDATE | Full toolchain, package, and exact-revision CI checks | Acceptance | Implemented software; CR-INTEGRATION | `[ ]` |
| CR-QUALIFY | Isolated installed-system and physical cleanup evidence | Acceptance | CR-F5 software; candidate packages | `[ ]` |
| CR-CLOSEOUT | Reviewed finding closure and original-gate reconciliation | Release gate | All applicable work and evidence above | `[ ]` |

## CR-BASE — Preserve the baseline and establish failing regressions

- [x] **CR-BASE-01:** Recheck HEAD, dirty state, applicable repository instructions, and current
  code before implementation. Record any differences from the reviewed baseline.
- [x] **CR-BASE-02:** Preserve F1–F8 descriptions, affected paths, reproduction conditions, and
  original task relationships in durable evidence linked to this backlog.
- [x] **CR-BASE-03:** Convert the isolated demonstrations for F1/F2/F3/F4/F6/F8 into meaningful
  repository regression tests whose corrected expectations fail against the baseline.
- [-] **CR-BASE-04:** Establish cleanup fixtures for F5: owned and foreign profiles/addresses,
  routes, forwarding baseline/lease, interrupted cleanup, and a stopped daemon.
- [x] **CR-BASE-05:** Record F7's concrete source/document discrepancies, preserving dated
  hardware evidence and distinguishing stale summaries from still-open acceptance.
- [x] **CR-BASE-06:** Confirm all automated fixtures are isolated from the host package database,
  system services, real Bluetooth controllers, and ordinary user networking.

**Acceptance:** Each finding has a durable reproduction or explicit static-evidence basis;
baseline failures demonstrate the reported condition, rather than merely asserting wording.

**Progress (2026-11-13):** acceptance met. F1/F2/F3/F4/F6/F8 have repository regressions that fail
on the baseline for the reported reason (committed disabled; each fix re-enables its own), F5 has
its static basis, and F7 its discrepancy list; see the evidence record. CR-BASE-04 stays open:
its interrupted-cleanup and stopped-daemon fixtures depend on the entry point CR-F5-01 designs.

## CR-F8 — Bind commands and versions to one owner

**Requirements:** OWN-1–OWN-4. **Original tasks:** P5-001/005.

- [x] **CR-F8-01:** Resolve the unique owner and use the same destination for compatibility
  negotiation, normal command delivery, and response handling.
- [x] **CR-F8-02:** Scope cached compatibility/subscription metadata to that owner. Preserve
  exact typed error classification and established major/minor compatibility rules.
- [x] **CR-F8-03:** Handle owner loss before dispatch safely; prevent automatic replay after a
  mutation might have been delivered or its reply lost.
- [x] **CR-F8-04:** Add a barrier-controlled A-to-API-99.0 owner-swap test during Version and
  assert the replacement receives zero Request calls.
- [x] **CR-F8-05:** Cover compatible replacement, relinquished-but-connected A, disappearance,
  queued old-owner events, and lost reply without mutation replay.
- [x] **CR-F8-06:** Review frontend/client call sites for assumptions that the version or stream
  remains valid across a well-known-name ownership change.

**Acceptance:** Incompatible replacement cannot receive a normal command under any tested
handover. Existing CLI/bridge compatibility and authorization tests remain green.

**Accepted (2026-11-13, `b3ba0bd`):** met; see [evidence](CODE_REVIEW_REMEDIATION_EVIDENCE.md#cr-f8--owner-bound-operations-2026-11-13).

## CR-F2 — Detect owner loss and reconnect observers

**Requirements:** WATCH-1–WATCH-4; OWN-4. **Original tasks:** P5-005; P11-001; P12-002.

- [x] **CR-F2-01:** Observe owner changes and bus closure independently of the Event stream,
  including reconciliation of an owner change during subscription setup.
- [x] **CR-F2-02:** Invalidate/terminate old-owner subscriptions and reject stale queued events;
  ignore unrelated bus-name changes.
- [x] **CR-F2-03:** Connect desktop and TUI watch loops to the new lifecycle result and expose
  disconnected/unavailable state without requiring an application Event.
- [x] **CR-F2-04:** Restore a fresh compatible owner-bound subscription and reread snapshots on
  return; retain bounded backoff and reject incompatible replacement.
- [x] **CR-F2-05:** Label retained UI data stale or unavailable while disconnected. Ensure
  consumer shutdown releases callbacks and watch resources.
- [x] **CR-F2-06:** Test idle stop/restart, direct replacement, incompatible return, bus loss,
  setup races, cancellation, and reconnect without duplicate listeners.
- [x] **CR-F2-07:** Measure the controlled two-second owner-loss bound and verify production
  backoff limits using injected timing rather than long or fragile sleeps.

**Acceptance:** Idle daemon loss reaches both actual watcher implementations; compatible return
refreshes state on the bounded retry schedule without a synthetic Event or manual refresh.

**Accepted (2026-11-13, `e283b4e`):** met. Both front ends now run the one shared
`blueroute_client::watch`, which is tested end to end on a private bus; see
[evidence](CODE_REVIEW_REMEDIATION_EVIDENCE.md#cr-f2--owner-loss-detection-and-watcher-recovery-2026-11-13).

## CR-F1 — Publish actual daemon changes

**Requirements:** EVENT-1–EVENT-6. **Original tasks:** P11-001/002; P12-002/007/010; P13-008.

- [x] **CR-F1-01:** Inventory snapshot fields/producers and choose a central publication path,
  bounded queue/coalescing policy, ordering semantics, and failure reporting.
- [x] **CR-F1-02:** Specify event/signal compatibility before adding wire variants. Reuse correct
  existing semantics where possible; do not fabricate unrelated state-change events.
- [x] **CR-F1-03:** Publish committed create/join-where-enabled/leave and rename changes after
  required persistence and public state updates.
- [x] **CR-F1-04:** Publish runtime capability/system/health changes and authenticated peer
  arrival, departure, and revocation from actual worker observation updates.
- [x] **CR-F1-05:** Publish client-link detail transitions even when aggregate health stays the
  same; cover observable discovery/list and diagnostics changes as applicable.
- [x] **CR-F1-06:** Emit outside state locks; preserve successful mutation results if signaling
  fails; keep the latest dirty state pending for bounded retry/reconciliation.
- [x] **CR-F1-07:** Close the initial snapshot/subscription race and refresh on every newly
  established compatible owner.
- [x] **CR-F1-08:** Add private-broker tests driven by real service mutations/worker updates,
  without manually calling `emit_event` to create the expected notification.
- [x] **CR-F1-09:** Cover rollback/no false success, unchanged samples, burst coalescing,
  saturation, emitter failure, independent observers, and the post-publication latency bound.

**Acceptance:** An already connected observer rereads current committed state after another
client or worker changes it. No producer can silently leave the final snapshot uninformed.

**Accepted (2026-11-13, `4de29b1`):** met with the user-chosen `StateChanged` signal (API 1.7);
see [evidence](CODE_REVIEW_REMEDIATION_EVIDENCE.md#cr-f1--publish-actual-daemon-changes-2026-11-13).

## CR-F3 — Preserve graph consistency

**Requirements:** GRAPH-1–GRAPH-4. **Original task:** P2-005; downstream P8/P9 prerequisites.

- [ ] **CR-F3-01:** Audit graph callers; document identical insertion, property update,
  incompatible rebinding, explicit membership, and boolean return semantics.
- [ ] **CR-F3-02:** Preserve attached membership on identical SegmentId reinsertion. Reject a
  changed NAP by default or implement a fully validated atomic replacement contract.
- [ ] **CR-F3-03:** Prevent stale membership on LinkId rebinding; preserve legitimate members
  still required by other links and any explicitly supported non-link membership.
- [ ] **CR-F3-04:** Validate endpoints, segment references, and NAP agreement before mutating
  any index. Failed operations must preserve a complete before/after graph snapshot.
- [ ] **CR-F3-05:** Test same-ID duplicate insertion, changed endpoints/segment/NAP, property-only
  updates, invalid changes, multiple links to a member, and removal after updates.
- [ ] **CR-F3-06:** Add downstream deterministic route/forwarding checks over the resulting
  graph; retain existing single-star, routed, redundant-path, and partition tests.

**Acceptance:** All graph representations agree after every supported mutation; invalid
replacement is atomic and deterministic; the original two replacement probes no longer fail.

## CR-F4 — Refresh operational hosting capabilities

**Requirements:** CAP-1–CAP-4. **Original tasks:** P6-001; P12-004/005; P13-004.

- [ ] **CR-F4-01:** Replace the immutable startup precondition with a fresh observation or a
  coherently refreshed, synchronized capability generation shared with operational decisions.
- [ ] **CR-F4-02:** Define freshness/failure behavior and review other latched copies, including
  control admission; distinguish stable configured policy from live runtime capability.
- [ ] **CR-F4-03:** Preserve fail-closed unknown/unsupported/error behavior and operation-time
  backend checks; do not mutate networking merely by refreshing capabilities.
- [ ] **CR-F4-04:** Test false/unknown-to-true recovery on the same daemon/operations instance;
  verify creation reaches the fake runtime without restarting the daemon.
- [ ] **CR-F4-05:** Test true-to-false, probe failure, and concurrent observation changes;
  unsafe creation must not reach the runtime and failure must preserve rollback guarantees.
- [ ] **CR-F4-06:** Verify readiness and observer notifications use compatible observations;
  preserve membership, key pins, ceilings, authorization, and production join restrictions.

**Acceptance:** Observed recovery unblocks eligible creation without restart; stale or failed
observations cannot grant unsafe capability. Hardware adapter-reset acceptance remains separate.

## CR-F5 — Clean up installation-owned runtime networking

**Requirements:** CLEAN-1–CLEAN-7. **Original tasks:** P15-005; P17-006.

- [ ] **CR-F5-01:** Record the maintenance entry point, removal ownership scope, writer
  quiescence/locking, resource order, timeout, and Debian failure/recovery behavior.
- [ ] **CR-F5-02:** Provide stopped-service cleanup through typed adapters without invoking
  networking shell tools or adding generic frontend privileges.
- [ ] **CR-F5-03:** Prevent daemon/startup/reconnect/control writers from restoring resources
  during removal. Retain ownership metadata and runtime forwarding lease until no longer needed.
- [ ] **CR-F5-04:** Enumerate and remove supported owned PAN, profile/interface/address/route
  resources; reject ambiguous/cross-installation ownership and preserve foreign/default state.
- [ ] **CR-F5-05:** Release forwarding according to its recorded baseline, preserving externally
  enabled forwarding. Do not add absent gateway/NAT/firewall behavior.
- [ ] **CR-F5-06:** Support NetworkManager-only cleanup while Bluetooth is unavailable; attempt
  independent safe work and retain unresolved ownership/retry evidence on partial failure.
- [ ] **CR-F5-07:** Wire remove/deconfigure/purge separately from upgrade/restart/abort paths;
  retain durable state on remove and delete it on purge only after cleanup succeeds.
- [ ] **CR-F5-08:** Stop swallowing cleanup failure. Return a useful non-secret failure and test
  Debian recovery/retry paths without deleting the executable or evidence prematurely.
- [ ] **CR-F5-09:** Add fake-backend cases for active/absent/replaced/foreign resources,
  concurrent writers, service absence, timeout, partial failure, and repeated cleanup.
- [ ] **CR-F5-10:** Extend isolated dpkg tests to assert cleanup invocation, ordering,
  failure propagation, remove-then-purge, direct purge, upgrade, and abort behavior.
- [ ] **CR-F5-11:** Verify scripts and cleanup executable embedded in the actual packages;
  preserve the v2-to-v3 migration/stable-identity upgrade test.
- [ ] **CR-F5-12:** Complete the installed-system/physical qualification in CR-QUALIFY before
  claiming no active networking remains after uninstall.

**Acceptance:** Package removal succeeds only after verified owned runtime state is absent;
foreign networking and retained user state remain intact. Full closure requires CR-QUALIFY.

## CR-F6 — Own asynchronous UI resources by effect generation

**Requirements:** UI-1–UI-3. **Original tasks:** P12-002/012.

- [ ] **CR-F6-01:** Replace shared-liveness ownership decisions with effect-local disposal or
  generation tracking for subscriptions, callbacks, loaders, and the refresh handler.
- [ ] **CR-F6-02:** Immediately unsubscribe late results for disposed setups exactly once;
  ignore old callbacks and fetch completions after replay or dependency replacement.
- [ ] **CR-F6-03:** Keep coalescing bounded per active generation so an obsolete request cannot
  block, replay, or overwrite the replacement API's current load.
- [ ] **CR-F6-04:** Test StrictMode replay with deferred subscription results in both orders,
  pending reads during unmount, rapid remount, and changed `api`/`subscribe` dependencies.
- [ ] **CR-F6-05:** Assert one listener while mounted and zero after unmount; late old events
  cause neither state changes nor new daemon reads. Test rejected setup and signal bursts.
- [ ] **CR-F6-06:** Keep StrictMode and all lint rules enabled; verify visible current data as
  well as unsubscribe counts.

**Acceptance:** The baseline `[0, 1]` unsubscribe leak becomes complete cleanup; old generations
cannot interfere with current view/link/loading behavior.

## CR-F7 — Reconcile documentation with validated behavior

**Requirements:** DOC-1–DOC-4. **Original task:** P17-003; acceptance references across P0–P19.

- [ ] **CR-F7-01:** Correct current API/schema/package summaries against source and built
  artifacts: reviewed baseline API 1.6, schema v3, core package with TUI, separate desktop package.
- [ ] **CR-F7-02:** Index P8-009 and audit other accepted decisions; distinguish accepted ADRs,
  design proposals, implemented software, and qualified product behavior.
- [ ] **CR-F7-03:** Reconcile stale TLS/enrollment/daemon/frontend summaries in SPEC, the
  original TODO, threat model, hardware/support tracker, and relevant operational guides.
- [ ] **CR-F7-04:** Describe owner/version/event guarantees, capability freshness, graph update
  semantics, and remove/upgrade/purge behavior after their implementations are accepted.
- [ ] **CR-F7-05:** Preserve historical evidence, stable original task IDs, the read-only TUI
  decision, closed Join, absent live routing, and disabled sharing boundaries.
- [ ] **CR-F7-06:** Link original affected tasks to remediation evidence without closing their
  unrelated product/hardware criteria; mark superseded summaries clearly.
- [ ] **CR-F7-07:** Complete a final source/document comparison at the candidate revision;
  remove dependence on temporary review artifacts and validate internal links.

**Acceptance:** No known current-state contradiction remains for these findings. Documentation
states which software, installed-system, physical, and product gates have actually passed.

## CR-INTEGRATION — Exercise corrected component boundaries

- [ ] **CR-INTEGRATION-01:** Run actual client and desktop/TUI watchers against a private broker
  with idle loss, compatible/incompatible replacement, queued events, and controlled owner races.
- [ ] **CR-INTEGRATION-02:** Drive real DaemonService requests/worker publications from one
  connection and verify a second observer refreshes without manually emitting the expected event.
- [ ] **CR-INTEGRATION-03:** Test publication/subscription startup races, burst coalescing,
  notification failure after commit, and client-link changes with unchanged aggregate health.
- [ ] **CR-INTEGRATION-04:** Combine fresh capabilities, a fake network runtime, service status,
  and published readiness in one unavailable-at-startup recovery scenario.
- [ ] **CR-INTEGRATION-05:** Check graph updates against routing/forwarding consumers and failed
  operations against complete before/after snapshots.
- [ ] **CR-INTEGRATION-06:** Exercise maintenance cleanup ordering with active owned/foreign
  resources and package lifecycle calls, including failure followed by retry.
- [ ] **CR-INTEGRATION-07:** Add the regressions to CI explicitly. Document any ignored broker
  tests' exact invocation, and verify no test requires an ordinary host network mutation.

**Acceptance:** Tests fail when the original boundary defects are deliberately restored and
pass with corrected behavior. Passing helper tests or mock notifications alone are insufficient.

## CR-VALIDATE — Qualify the exact software candidate

- [ ] **CR-VALIDATE-01:** Run Rust formatting, locked workspace check, strict all-target Clippy,
  and the full locked workspace test suite at the candidate SHA.
- [ ] **CR-VALIDATE-02:** Run daemon `experimental-join` software tests while verifying ordinary
  package/default builds retain the closed production join gate.
- [ ] **CR-VALIDATE-03:** Run all five existing isolated broker checks plus new owner/watch/
  publication regressions and the Python socket-tooling tests.
- [ ] **CR-VALIDATE-04:** Run pinned frontend install, format/lint/type checks, component/StrictMode
  tests, and production build; validate any changed theme token contrast.
- [ ] **CR-VALIDATE-05:** Build both Debian packages, inspect manifests/dependencies/embedded
  frontend, and execute the packaged lifecycle and isolated upgrade/migration/removal/purge tests.
- [ ] **CR-VALIDATE-06:** Record actual commands, test results, revision, package hashes, and
  limitations; inspect exact-head CI through Ralph Bridge per repository procedure.
- [ ] **CR-VALIDATE-07:** Recheck authorization, pin/exporter/revocation, rollback, foreign-state
  preservation, stable identity, schema migration, no replay, and disabled sharing regressions.

**Acceptance:** Required software checks and exact-revision CI pass. Any hardware deferral is
explicitly recorded; software success alone does not complete CR-F5 or CR-CLOSEOUT.

## CR-QUALIFY — Validate active-network cleanup in an isolated system

- [ ] **CR-QUALIFY-01:** Prepare a disposable VM/test machine with actual NetworkManager,
  systemd, and candidate packages; capture versions, source SHA, and package hashes.
- [ ] **CR-QUALIFY-02:** Create a supported owned bridge/address/profile and foreign comparison
  fixtures. Remove while active; inspect actual backend and kernel state independently.
- [ ] **CR-QUALIFY-03:** Repeat with daemon already stopped, Bluetooth unavailable, partial
  backend failure/recovery, repeated cleanup, remove-then-purge, and direct purge.
- [ ] **CR-QUALIFY-04:** Prove identity/membership survive ordinary remove, purge occurs only
  after cleanup, and upgrade/restart do not accidentally execute removal policy.
- [ ] **CR-QUALIFY-05:** Verify no owned active profiles/addresses/routes/interfaces remain,
  foreign/default state is preserved, and the forwarding baseline is respected with its lease.
- [ ] **CR-QUALIFY-06:** On an explicitly designated Bluetooth test rig, qualify owned PAN
  attachment/registration teardown and compare foreign Bluetooth/network state before/after.
- [ ] **CR-QUALIFY-07:** Record real adapter recovery and connected-UI observation where those
  hardware claims are made; distinguish them from fake-runtime capability tests.
- [ ] **CR-QUALIFY-08:** Store reproducible evidence and unresolved cases in a versioned record
  linked here and in the hardware tracker. If equipment is deferred, keep this task open.

**Acceptance:** Observed installed-system state supports the cleanup claims, and physical PAN
claims have identified hardware evidence. Synthetic dpkg roots are not sufficient qualification.

## CR-CLOSEOUT — Review all findings and reconcile original gates

- [ ] **CR-CLOSEOUT-01:** Review each F1–F8 fix against the companion specification, original
  reproduction, adverse cases, security invariants, and integration evidence.
- [ ] **CR-CLOSEOUT-02:** Confirm all eight findings have source/commit, regression or
  documentation-comparison results, software outcome, and required qualification evidence linked below.
- [ ] **CR-CLOSEOUT-03:** Recheck affected original tasks; retain every unsatisfied physical,
  product, join/routing, support, and release acceptance criterion.
- [ ] **CR-CLOSEOUT-04:** Resolve remaining contradictions or blockers before marking the
  remediation COMPLETE. Record a software milestone separately if hardware remains deferred.

**Acceptance:** Every applicable criterion is evidenced, all eight findings are resolved, and
the final status does not imply completion of the rest of the product roadmap.

## Evidence ledger

Fill entries as work is qualified. Link concrete records; do not replace evidence with a checkmark.

| Task / finding | Fix SHA / source | Regression and software results | Installed-system / physical evidence | Remaining gate | Status |
| --- | --- | --- | --- | --- | --- |
| CR-BASE | Reproduction tests at the CR-BASE commit | Baseline failures recorded in [evidence](CODE_REVIEW_REMEDIATION_EVIDENCE.md#cr-base--baseline-and-reproductions-2026-11-13) | Preserve recorded baseline scope | CR-BASE-04 F5 fixtures (with CR-F5-01) | IN PROGRESS |
| CR-F8 / F8 | `b3ba0bd` | 5 `owner_race` tests pass (2 fail with the baseline client); all checks and bus suites green | Not required for the owner-race defect | Exact-head CI recorded under CR-VALIDATE | COMPLETE |
| CR-F2 / F2 | `e283b4e` | `owner_watch` (3), `watcher` (5), setup race in `owner_race`; UI stale-label tests; all checks green | Private broker sufficient for the reported defect | Exact-head CI recorded under CR-VALIDATE | COMPLETE |
| CR-F1 / F1 | `4de29b1` | `state_events` (7), daemon-to-watcher, link-phase and peer-snapshot tests; all checks green | Radio claims remain separate | Exact-head CI recorded under CR-VALIDATE | COMPLETE |
| CR-F3 / F3 | Pending | Pending | Not required for pure graph consistency | Atomic mutation and consumer invariants | NOT STARTED |
| CR-F4 / F4 | Pending | Pending | Adapter behavior if claimed | Same-instance software recovery; preserve hardware gates | NOT STARTED |
| CR-F5 / F5 | Pending | Pending | Required: CR-QUALIFY | Owned cleanup, foreign preservation, failure/retry | NOT STARTED |
| CR-F6 / F6 | Pending | Pending | Not required for effect ownership | No listener or stale-generation leak | NOT STARTED |
| CR-F7 / F7 | Pending | Pending | Preserve existing records | Current summaries and final acceptance accuracy | NOT STARTED |
| CR-INTEGRATION | Pending | Pending | Isolated-system results where relevant | Boundary regressions included in CI | NOT STARTED |
| CR-VALIDATE | Pending | Pending | No physical claim from CI | Exact candidate software/package qualification | NOT STARTED |
| CR-QUALIFY | Pending | Pending | Pending | All required installed-system and physical checks | NOT STARTED |
| CR-CLOSEOUT | Pending | Pending | Pending where required | Full finding and original-gate review | NOT STARTED |

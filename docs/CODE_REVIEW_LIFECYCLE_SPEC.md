# Code Review Lifecycle Remediation Specification

**Status:** Proposed; no fix or acceptance is established by this document.  
**Reviewed baseline:** `75c1a01746557bb13f525a55642b497e3bd5f81d` (`master`).  
**Execution backlog:** [CODE_REVIEW_LIFECYCLE_TODO.md](CODE_REVIEW_LIFECYCLE_TODO.md).  
**Related requirements:** [follow-up specification](CODE_REVIEW_FOLLOWUP_SPEC.md),
[follow-up backlog](CODE_REVIEW_FOLLOWUP_TODO.md), [follow-up evidence](CODE_REVIEW_FOLLOWUP_EVIDENCE.md),
[product specification](SPEC.md), and [original backlog](TODO.md).

## 1. Purpose and review findings

Resolve both P2 findings from the review of the follow-up implementation, cover the missed state
transitions with production-path regressions, and correct premature software-completion claims.
This specification supplements the earlier OWNER/REMOVE requirements. It preserves earlier
working fixes, evidence, task identities, and installed-system/physical acceptance gates.

| Finding | Trigger and observed result | Affected implementation and earlier tasks |
| --- | --- | --- |
| RFU-01: returning owner bypasses retirement | A emits an event, transfers the service name to B, and reacquires it before its event-time Version reply is consumed. With reply and owner notices ready together, the subscription delivers A's old event; only the next call returns OwnerLost. Reproduced 10/10 private-broker runs. | `crates/blueroute-client/src/lib.rs`, especially `next_change`; FU-02-02/03, FU-INTEGRATION-02; OWNER-1..4 |
| RFU-02: unknown active role bypasses client cleanup | A valid Member/None membership and matching client record become `NetworkScope { client: false, host_hint: None }`. Cleanup without Bluetooth deletes owned profile proof and reports success without observing possible PANU work. | `apps/daemon/src/removal.rs`, especially `removal_scope`; FU-04-01/02/03/06/07/09, FU-INTEGRATION-03; REMOVE-1..5 |

The recipes here are durable descriptions. Implementation must add repository tests and versioned
evidence; acceptance must not depend on temporary review programs or logs under `/tmp`.

### Scope boundaries

- Preserve owner-bound command delivery, compatibility checks, mutation no-replay, typed errors,
  bounded reconnect backoff, authorization, authenticated identity, pins, revocation, and rollback.
- Keep Bluetooth/networking policy in daemon/Linux adapters and shared client code. Frontends
  retain narrow payload-free notifications; TUI remains read-only.
- Keep production join closed and Internet sharing disabled. Do not add forwarding, routing,
  NAT, firewall, or general administrative mutation features to solve these defects.
- Preserve API 1.7 and membership schema v3 unless a separately justified compatibility decision
  is recorded. A client-local lifecycle model and internal cleanup scope should suffice.
- Retain the accepted proof-retention policy: existing NetworkManager profile/address evidence
  remains until dependent teardown is verified. Do not create a new durable cleanup journal
  merely to repair scope classification.
- Device-disappearance probing returned a safe failure with proof retained; no additional BlueZ
  defect was established. Cooperative cancellation's inability to preempt a synchronous kernel/
  filesystem stall remains a limitation, not a newly reproduced defect in this scope.

## 2. LC-OWNER: Bind subscriptions to an ownership period

### Required behavior

**EPOCH-1 — Continuous ownership:** A subscription belongs to the continuous ownership period
in which it was installed, not just to a unique D-Bus connection name. Once A relinquishes
`org.blueroute.Service1`, that subscription is retired even if the same connected A later
reacquires the name. An A→B→A or A→unowned→A sequence cannot revive the old subscription.

**EPOCH-2 — Delivery ordering:** Define a delivery decision point that accounts for lifecycle
transitions queued before it. A ready event, StateChanged signal, or Version reply must not
bypass an already queued retirement. Current-name equality after Version is insufficient to
prove uninterrupted ownership. Handle retirement before signal selection, during validation,
and while final ownership verification is awaiting a reply. Merely reversing one `future::or`
branch order is insufficient unless the full decision path and queued-message ordering are proven.
A change occurring after a valid delivery decision does not retroactively invalidate that result.

**EPOCH-3 — Terminal and cancellation-safe:** Retirement ends the old subscription with the
existing typed OwnerLost outcome and rejects all its selected/queued later items and late replies.
Cancellation must not consume and forget a retirement or reset its recorded state. A fresh
subscription can bind to returning A only after fresh compatibility validation and snapshot
reconciliation. Distinguish a transient verification failure from demonstrated owner loss.

**EPOCH-4 — Watcher behavior and progress:** Shared watcher, desktop, and TUI must report the
disconnected/stale boundary before accepting changes from a newly established ownership period.
With an actively polled consumer on a healthy private broker, preserve the two-second owner-loss
bound and bounded reconnect backoff. A deliberately paused consumer must return retirement
promptly on resumption, before any obsolete item; wall-clock bounds do not imply progress while
the caller never polls. Preserve subscribe-before-read or subscribe-and-reread handoff.

**EPOCH-5 — Compatibility and resources:** Retain unique-owner Version/Request destinations,
major/minor compatibility rules, no mutation replay after possible delivery, and zero ordinary
Requests to an incompatible successor. Healthy subscriptions must continue delivering valid
events without busy Version polling, unbounded buffering, leaked monitor tasks, or a new wire
epoch field. Resource lifetime/cancellation must follow the subscription's owner.

### Required regression scenarios

Use barriers/acknowledgements to control the server, broker ordering, and consumer polling.
Timeouts bound assertions; arbitrary sleeps must not be the mechanism that establishes the race.
Exercise both typed Event and StateChanged handling where they share delivery logic.

| Case | Required observation |
| --- | --- |
| A→B→A inside event-time Version; consumer resumes after reply and notices are ready | First result is OwnerLost; no old item; later calls remain retired; fresh subscription hears newly emitted A state. |
| A→unowned→A, with a queued old signal before selection | Old subscription retires even though GetNameOwner again returns A. |
| Retirement becomes ready with Version or during final verification | Retirement cannot be deferred to the next `next_change` call. |
| Held/late Version reply, cancellation while awaiting validation | Loss progresses independently of the reply; cancellation cannot erase the transition. |
| Compatible B, incompatible B, and disappearance without replacement | Existing reconnect, refusal, no-replay, and no-successor behavior remains correct. |
| Healthy continuous A; transient verification error; change after delivery decision | Valid events still arrive; transient errors do not permanently retire a healthy subscription. |

## 3. LC-CLEANUP: Preserve unknown active membership state

### Classification contract

The store deliberately accepts Member/None as an **unknown** role. Cleanup must retain enough
membership state to distinguish it from NotMember/None, which can represent a completed leave.

**SCOPE-1 — Explicit state model:** Replace or augment the boolean cleanup classification with
an explicit unknown-active outcome. Do not derive host-only safety from `role != Some(Client)`.
Document all accepted membership/record combinations before destructive work.

| Persisted membership | Client record | Cleanup classification |
| --- | --- | --- |
| Member/Host | Absent or stale record | Confirmed host; retain existing host-only cleanup behavior. A record alone does not override a known role. |
| Member/Client | Present or missing host hint | Client obligations; retain existing observed-absence, disconnect, and unresolved outcomes. |
| Member/None | Present or absent | Unknown active role; fail unresolved teardown and preserve dependent proof. |
| NotMember/None | Absent or stale record | Preserve successful completed-leave/stale-record cleanup; do not invent a client obligation solely from that stale record. |
| No membership row | Client record references the network | Missing state is not a confirmed completed leave or Host role. Apply an explicit conservative unresolved policy; do not silently create a host-only scope. |
| Invalid, unreadable, or unsupported stored state | Any | Preserve existing fail-closed validation; do not rewrite it into a successful cleanup classification. |

**SCOPE-2 — Honest outcome and proof lifetime:** An unknown active role cannot count as proven
absent client work, even with zero inventoried NetworkManager resources. Return failure with a
bounded unresolved category/count. Preserve membership, client hints, and owned profile/address
proof needed for later reconciliation. Do not clear records or relabel the network Host/NotMember
to make removal succeed. Logs must not expose device paths, secret material, or arbitrary records.

**SCOPE-3 — Ownership boundaries:** Hints identify observations, not ownership. Continue requiring
verified NetworkId/profile/address or retained attachment evidence before disconnecting a link.
Unknown-role handling must neither disconnect foreign links nor erase foreign profiles, default
routes, non-owned addresses, another installation's resources, or externally enabled forwarding.
An unknown active role without a hint is still unknown; a valid hint alone cannot resolve it.

**SCOPE-4 — Independent progress:** An unresolved network must not prevent safe cleanup of
unrelated confirmed networks. Keep the instance lock/removal marker, restore-worker quiescence,
existing 60-second cleanup bound, reobservation, timeout/error behavior, and idempotence. Avoid
turning one unknown role into a global pre-inventory error that stops all independent work.

**SCOPE-5 — Evidence-based recovery:** Retried cleanup must re-read/reobserve authoritative state.
After independently established role/ownership facts resolve the ambiguity, normal owned client
or host cleanup can finish safely and repeat idempotently. Bluetooth becoming available alone
does not prove a role. Tests may seed verified recovered state through production stores; they
must not require a new production role-editing API or teach operators to guess roles. Document
the evidence required for recovery and preserve the unresolved state until it exists.

**SCOPE-6 — Helper and package outcome:** Carry unresolved scope through the production cleanup
report, helper exit status, and remove/deconfigure/direct-purge paths. Removal failure must retain
retry evidence and prevent ordinary purge from deleting it. Preserve upgrade and abort/retry
semantics and the legitimate stale-record fix. Internal type changes must not silently change
wire/storage formats or add a production shell networking path.

### Required regression scenarios

Build the reported Member/None plus matching client-record case with the production membership
and client-record stores, not a hand-written scope that already embeds the expected answer.
Require failure and retained proof with Bluetooth unavailable, with Bluetooth available but role
still unknown, and with no device hint. Include another confirmed network and foreign fixtures
to demonstrate independent cleanup and foreign preservation. Cover the remaining classification
table rows, missing/conflicting proof, interruption/retry, a verified recovered role, and a repeated
successful run. Assert the actual report and helper status as well as resource/record preservation.

## 4. Integration and validation

**VERIFY-1 — Sensitive regressions:** New corrected expectations must fail against this reviewed
baseline for the reported reasons and pass with the fix. Record safe negative controls in a
temporary checkout or equivalent isolated setup. Never leave a restored defect in tracked code.
Convert the review recipes into maintained tests; ten reproductions of a bug do not establish
that a correction works.

**VERIFY-2 — Real boundaries:** Test production subscription/watcher paths on a private broker,
including desktop/TUI adapters. Test production store→scope→cleanup→report→helper outcome and
maintainer/isolated-dpkg failure/retry ordering. State explicitly when a backend or installed
helper is mocked. Retain a test showing the actual adapter fails safely on UnknownObject; do not
turn that already safe behavior into an absence/success shortcut.

**VERIFY-3 — Exact candidate:** Run pinned Rust and frontend checks, explicit ignored broker
suites, Python tooling, both package builds, and lifecycle/migration checks listed in the TODO.
Record exact SHA, commands, pass/fail/ignored counts, environment, package hashes and CI result.
Inspect exact-revision CI with git and the GitHub CLI (`gh run`), recording run IDs
(maintainer decision, 2026-11-14). The prior review's 901 Rust, 48 broker and 43 frontend passes are a
baseline, not fixed expected counts or evidence that these two defects are closed.

**VERIFY-4 — Preserve existing fixes:** Re-run slow-sampler progress, bounded cooperative
shutdown, startup handoff, graph import/atomicity, ordinary A→B retirement, normal Client
cleanup/recovery, NotMember stale-record, UI effect-generation and security-boundary regressions.
Ensure newly added ignored tests are actually scheduled in CI.

## 5. Evidence, documentation, and completion

**CLOSE-1 — Reconcile claims:** Append dated corrections to the follow-up backlog/evidence and
affected current summaries, cleanup design, client lifecycle documentation and qualification
procedure. Requalify FU-02, FU-04, FU-INTEGRATION, and FU-CLOSEOUT software claims against these
findings. Preserve historical results and the original CR/FU/P task IDs. The companion lifecycle
TODO is the current status authority for RFU-01/02; link it during implementation.

**CLOSE-2 — Separate acceptance layers:** Both defects have software regressions that can close
without physical equipment. A software milestone still requires all software criteria satisfied.
Full cleanup/release acceptance retains FU-QUALIFY, CR-QUALIFY-01..08 and original hardware/product
gates. Installed-system qualification must include the unknown-role failure/proof-retention case
on the adapter-free fixture. Physical PAN claims still require the designated Bluetooth rig.
Missing equipment leaves those gates open and must not block independent software correction.

**CLOSE-3 — Durable traceability:** Record source/fix SHA, each requirement's task/test/evidence
mapping, corrected baseline failure, final result, remaining limitations, and independent review
outcome in a versioned evidence record created during implementation. Correctness, procedure
readiness, and executed qualification must have separate outcomes. No overall COMPLETE claim is
valid while required installed-system or physical acceptance remains unsatisfied.

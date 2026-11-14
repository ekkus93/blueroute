# Code Review Queue and Recovery Remediation Specification

**Status:** Proposed; no implementation or acceptance is established here.  
**Reviewed baseline:** `f957d80207930ccb481bec8f23cefd8bb2506cde`.  
**Execution backlog:** [CODE_REVIEW_QUEUE_RECOVERY_TODO.md](CODE_REVIEW_QUEUE_RECOVERY_TODO.md).  
**Related documents:** [lifecycle specification](CODE_REVIEW_LIFECYCLE_SPEC.md),
[lifecycle backlog](CODE_REVIEW_LIFECYCLE_TODO.md), [lifecycle evidence](CODE_REVIEW_LIFECYCLE_EVIDENCE.md),
[product specification](SPEC.md), and [original backlog](TODO.md).

## 1. Purpose, findings, and boundaries

Resolve the two findings from the lifecycle review, add regressions for the missed transitions,
and reconcile software-completion claims with evidence. These requirements supplement EPOCH and
SCOPE requirements; they preserve existing working fixes and earlier task identities.

| Finding | Durable reproduction and impact | Affected paths / earlier tasks |
| --- | --- | --- |
| RLC-01, P2: ownership-queue saturation blocks final verification | Pause a consumer after its event-time Version request. Send the reply, then complete 40 release/reacquire cycles, queuing 80 owner notices. On resumption, the final GetNameOwner waits behind a full queue that the subscription is no longer draining. No OwnerLost arrives within 2.3 seconds; another call on that connection also stalls. Dropping the subscription restores progress. Reproduced 3/3 runs; a 20-cycle control retires normally. | `crates/blueroute-client/src/lib.rs`, final `current_owner` await and `ended_before`; LC-OWNER-02/05/06, LC-INTEGRATION-01; EPOCH-2/4 |
| RLC-02, P3: manual recovery retains an unresolved client record | After independently completing PAN/NM cleanup for Member/None plus a matching client record, follow the administrator guide's membership-deletion branch. The retained client record becomes OrphanClientRecord; removal still exits 1. Also removing the matching record permits success. | `docs/ADMINISTRATOR.md`, “Removal with an unresolved role”; LC-CLEANUP-07 and LC-CLOSEOUT; SCOPE-5 |

The existing qualification procedure already removes both records in the combined recovery case.
The unknown-role classifier correctly refuses uncertainty; fixing the guide must not weaken it.
The review also identified a coverage gap: notices after a valid delivery decision are reasoned
about but not directly exercised. Cover that boundary while repairing queue progress.

### Preserved constraints

- Keep owner-bound Version/Request destinations, compatibility rules, typed errors, terminal
  retirement, mutation no-replay, authorization, authenticated identities, pins and revocation.
- Keep privileged networking in daemon/Linux adapters and shared clients. Frontends retain narrow
  payload-free notifications; TUI stays read-only. Production join stays closed and sharing off.
- Preserve API 1.7, membership schema v3, explicit cleanup classes, ownership proof retention,
  instance locking, writer quiescence, foreign-resource preservation and the cleanup run bound.
- Do not add a role-editing API, new durable cleanup journal, wire epoch field, or routing/NAT/
  forwarding features. Existing sampler, startup, graph and UI effect-generation fixes must pass.
- Use maintained repository regressions and versioned evidence. Temporary review artifacts are
  background material, not an execution dependency or acceptance record.

## 2. QR-QUEUE: Preserve progress with bounded ownership queues

The pinned zbus 5.19 connection reader stops reading when a receiver queue fills. Receive-order
reasoning must therefore include progress of every receiver this subscription owns. The reviewed
matching signal queue defaults to 64 entries; a larger queue alone only moves the failure threshold.

**QUEUE-1 — No circular wait:** Resolve already queued period-ending notices before requiring
another bus round trip, and keep ownership observation progressing while final verification is
pending. A ready Version reply must not divert execution into an await whose response is blocked
by this subscription's unread notices. Account for signal selection, Version, final verification,
and their error paths together; document the capacity and scheduling argument.

**QUEUE-2 — Observable bound:** With a healthy private broker and an actively polled consumer,
report the existing typed OwnerLost within two seconds of the relevant ownership loss. For a
consumer intentionally paused while the burst accumulates, measure from resumption. Require the
loss before any obsolete item. The contract does not require progress while the caller is paused
or shorten physical Bluetooth observation intervals.

**QUEUE-3 — Preserve the delivery decision:** Retain the established receive-sequence decision
contract. A period-ending notice before the final verification reply prevents delivery; a notice
after a valid decision does not retroactively withdraw that permitted delivery, and retirement
must be retained for the next call. Returning A still needs a fresh subscription. A simple race
that always prioritizes notices must not discard the distinction between these two orderings.

**QUEUE-4 — Shared connection and resource lifetime:** Terminal retirement and dropping a
subscription must release or otherwise safely settle its owned receivers, so retaining an old
retired handle cannot keep unrelated calls on the shared connection blocked. Preserve other
active subscriptions. Cancellation must not consume and forget a retirement or leave an orphan
monitor. Keep buffering, concurrency and background work bounded; no busy polling, detached
permanent drainers, or unbounded per-notice tasks.

**QUEUE-5 — Regression matrix:** Test typed Event and StateChanged with below-capacity, at-boundary
and above-capacity bursts, including the 40-cycle reproduction. Establish reply/burst completion
using barriers or independent server acknowledgements that do not depend on the blocked client
reader. Test cancellation/resumption, a retained retired handle, drop, fresh subscription and a
second call on the same connection. Require the second call to complete within two seconds in
the controlled fixture after retirement, without requiring an extra caller-side drop to recover.
Do not use an indefinitely growing producer as an assertion mechanism.

**QUEUE-6 — Ordering and compatibility evidence:** Add direct coverage for pre-decision and
post-decision notices. If a broker cannot place the post-decision window deterministically, use
a focused internal test seam/state model for that ordering and retain the real-broker saturation
test. Preserve held/late reply, transient verification error, healthy owner, A→B→A, A→unowned→A,
incompatible successor, no-successor and no-replay regressions. Record the proof boundary and
pinned dependency assumptions so a zbus upgrade cannot silently invalidate them.

## 3. QR-RECOVERY: Complete the documented manual recovery

Manual recovery is an explicit administrator action after independent observation. It is distinct
from automatic cleanup, which must continue failing while a role or ownership obligation is
unresolved. No production code should silently erase uncertainty to satisfy a recovery test.

**RECOVER-1 — Complete record transition:** Align the administrator guide with qualification
Part A. With the service stopped and writers quiesced, identify the full NetworkId, verify PAN
teardown/absence, and remove only that network's proven owned NetworkManager state first. Then
remove the intended stale membership/peer rows and clear the client record **only if it names
that same NetworkId**. Removing membership while retaining its matching client record creates a
new unresolved orphan and is not completed recovery.

| Initial durable state | Required record handling after verified resource cleanup |
| --- | --- |
| Member/None plus matching client record | Remove that network's membership/peer rows and the matching client record. |
| Member/None without a matching record | Remove that network's membership/peer rows; preserve any client record naming another network. |
| Client record without membership | Clear the matching orphan record after its network's resource obligations are independently resolved. |
| Confirmed NotMember/completed leave | Retain existing successful stale-record cleanup behavior; do not introduce an unknown-role obligation. |

**RECOVER-2 — Safe scope and postconditions:** Preserve unrelated memberships, peers, identity,
keys, records and networking. A hint or interface name alone does not authorize destruction.
When teardown or record ownership cannot be established, retain proof and failure. After complete
targeted recovery, re-run cleanup and require success with no unresolved target scope and no
increase in foreign-resource counts caused by abandoned owned resources. Do not relabel an
unknown network Host/Client or delete unrelated data to obtain that outcome.

**RECOVER-3 — Test the actual workflow:** Seed Member/None and client records using production
stores. Model independently completed PAN/NM cleanup with an isolated backend, then perform the
documented record-deletion sequence. Require a deliberately incomplete membership-only deletion
to remain failed as OrphanClientRecord, complete matching-record deletion to succeed, and repeat
success to be idempotent. Cover no matching record, an unrelated record/network, existing orphan,
and confirmed completed leave. Keep existing tests that restore a verified role; they exercise a
different recovery and cannot substitute for this workflow.

**RECOVER-4 — Consistent guidance:** Reconcile administrator steps, cleanup design, qualification
procedure, current lifecycle summaries and evidence. Explain which actions are manual, what
establishes their preconditions, and what the retry should report. Preserve the distinction
between procedure readiness, software simulation and real installed-system/physical execution.

## 4. Validation, evidence, and completion

**ACCEPT-1 — Sensitive maintained tests:** Convert both review recipes into corrected repository
regressions that fail at this baseline for the intended reason and pass with the correction.
Include the queue control and membership-only negative case. Use private brokers, fake networking
and temporary state under ignored workspace `tmp/`. Run safe negative controls in an isolated
checkout/target; never restore a defect in tracked files and leave it behind.

**ACCEPT-2 — Production boundaries and exact candidate:** Exercise the production shared watcher
and desktop/TUI adapters, production store→scope→cleanup→helper outcome, and isolated packaged
script failure/retry paths. State which backend/helper layers are mocked. Run pinned toolchain,
frontend, broker, package and migration checks in the companion TODO. Record exact SHA, commands,
pass/fail/ignored counts, package hashes and exact-revision CI through the repository-approved
tooling. Preserve the lifecycle backlog's dated CI decision; do not treat historical green counts
as fixed future expectations or evidence these findings are resolved.

**ACCEPT-3 — Reconcile completion claims:** Append dated corrections to LC-OWNER, LC-CLEANUP,
LC-INTEGRATION and LC-CLOSEOUT outcomes and related summaries/evidence. Preserve earlier valid
results and stable LC/FU/CR/P IDs. Link this backlog as the current RLC-01/02 authority during
implementation. Map all 14 requirements to source, tasks and acceptance evidence, including the
post-decision coverage gap. A passing existing suite is insufficient closure by itself.

**ACCEPT-4 — Separate milestones and gates:** Both reported defects have software acceptance
that can proceed without equipment. Software completion requires corrected behavior, sensitive
tests, integration, validation, consistent documentation and review. Full cleanup/release claims
retain LC-QUALIFY, FU-QUALIFY, CR-QUALIFY-01..08 and original hardware/product gates. The revised
installed-system recovery procedure must preserve both-record deletion and foreign comparisons;
physical PAN claims require the existing designated rig. Record equipment deferral and continue
independent work. Never infer executed qualification from a revised procedure or mocked package
helper, and never call full remediation COMPLETE while required qualification remains open.

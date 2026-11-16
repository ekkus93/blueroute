# Code Review Reconnect Dispatch Specification

**Status:** Proposed; this document establishes no implementation or acceptance.  
**Finding:** RRC-01, P2 — an expired reconnect can send a new Version call.  
**Reviewed baseline:** `0257bcbf11f37276771a477e612529fc3e10f155` (2026-11-16).  
**Execution backlog:** [CODE_REVIEW_RECONNECT_DISPATCH_TODO.md](CODE_REVIEW_RECONNECT_DISPATCH_TODO.md).  
**Related requirements:** [reconnect specification](CODE_REVIEW_RECONNECT_SPEC.md),
[reconnect backlog](CODE_REVIEW_RECONNECT_TODO.md), [reconnect evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md),
[product specification](SPEC.md), and [original backlog](TODO.md), P5-005.

## 1. Problem, evidence, and scope

The earlier RQR-01 fix bounds owner/version negotiation and retry delays, rejects late outcomes,
and preserves informational state. One dispatch boundary remains incorrect. In
`crates/blueroute-client/src/lib.rs`, `negotiate_within` polls the attempt before its deadline
timer. If `validated_owner` is suspended in `current_owner`, a ready GetNameOwner reply can
advance into `negotiate_version_at` when the caller resumes after expiry. Version is sent before
the timer or post-outcome expiry check returns ReconnectTimeout.

This violates existing DEADLINE-3 and the public promise that nothing new is sent after expiry.
The observed impact is additional read-only bus work. The review did not demonstrate mutation
replay, a stale informational version, or successful completion after expiry in this scenario.
The original held-Version hang, late-success behavior, and uncapped retry delay remain fixed.

### Reproduction retained from the review

Use a private session broker, a scripted daemon, and the actual public client:

1. Construct the client; its initial Version exchange completes, recording API 1.7.
2. Poll `reconnect(Duration::from_millis(50))` to dispatch GetNameOwner, then leave it unpolled.
   Observe that lookup's reply on the client connection and establish that reconnect has not
   dispatched Version. The daemon's total Version count remains one.
3. Wait beyond expiry; the review resumed at approximately 110.5 ms, with Version count still one.
4. Poll reconnect once. It returns the typed 50 ms ReconnectTimeout, but sends another Version;
   the daemon's total becomes two. The corrected expectation is that it remains one.

All three final review runs failed that corrected assertion; the timely-owner control passed
three times. The approximately 161 ms final observation includes 50 ms after the result and is
not reconnect's return latency. An isolated timer-first copy passed both probes, supporting the
polling-order diagnosis; that experiment was not a production fix or full candidate validation.
These steps must become maintained tests and versioned evidence, independent of ignored review
artifacts or machine-specific paths.

### Associated coverage and acceptance gaps

The delayed-owner seam holds lookup pending while the timeout keeps being polled. Existing
late-reply broker cases suspend only after Version has already been dispatched. Neither checks
an owner reply becoming ready while reconnect is unpolled past expiry.

The review classified RC-DEADLINE-03 and RC-CLOSEOUT-04 as Incorrect, and RC-TEST-02 and
RC-CLOSEOUT-01/03 as Partial. Parent RC-DEADLINE, RC-TEST and RC-CLOSEOUT are Partial. The audit
retained all 26 leaves: 21 Verified, 3 Partial, 2 Incorrect; its full inventory had 47 entries.
This specification covers the code defect, missing boundary coverage, and unsupported closure
claims as one correction, preserving the earlier working behavior and historical results.

## 2. Required dispatch and deadline behavior

**DISPATCH-1 — Preserve one budget:** Use the existing monotonic deadline starting on the first
poll, across lookup, proxy setup, Version and retry delay. Retain zero-budget behavior, capped
backoff, existing retry classification, and non-panicking handling of an unrepresentable instant.
Do not restart the budget when a stage, retry or poll begins.

**DISPATCH-2 — Guard resumed work:** At every poll that resumes pending reconnect negotiation,
check the operation deadline before polling that negotiation onward. At or after expiry, return
ReconnectTimeout on that poll, without advancing lookup, proxy setup, Version or a retry.
The rule must not depend on an attempt-first race eventually reaching its timer branch.

**DISPATCH-3 — Guard stage transitions:** Before initiating a new owner lookup or Version call,
check the same deadline at the dispatch decision. A poll that starts in time but reaches the
next stage after expiry must not start that stage. Document the decision point: calls dispatched
before expiry may finish remotely afterwards; wall-clock time passing while the caller is
unpolled does not require remote cancellation. Do not claim atomic control over OS scheduling
or socket delivery after a valid dispatch decision.

**DISPATCH-4 — Preserve outcome precedence:** Keep the post-negotiation expiry decision as well
as dispatch guards. If a poll starts before expiry but its result is decided afterwards, expiry
wins over compatible success, incompatible response or another result. Timely compatible success
updates `server_version`; timely incompatibility stops immediately. A ready reply must never
turn an expired operation into success or extend its budget.

**DISPATCH-5 — Preserve typed errors and truthful context:** Return the existing
`ClientError::ReconnectTimeout { timeout, last_error }`, with the requested duration and
Unavailable classification. Retain useful bus failures already observed by this operation.
When a poll begun in time produces an error after expiry, retain its available diagnostic without
calling it a successful answer. When expiry is observed before polling pending work, a coarse
pending-negotiation description is sufficient; do not poll expired work to obtain better text.
Never fabricate a transport failure or make authorization/compatibility decisions from text.

## 3. Implementation and compatibility constraints

**PRESERVE-1 — Keep owner and command safety:** Retain unique-owner Version/Request addressing,
major/minor compatibility rules and command no-replay. Reconnect sends zero ordinary Request
calls, including to a replacement or incompatible daemon. Scope the change to reconnect's
private deadline path; normal request/subscription validation must retain its existing semantics.

**PRESERVE-2 — Keep state and resource lifetime:** Timeout, caller cancellation and late replies
leave `server_version` unchanged. Release operation-owned futures without background retries,
busy loops, leaked monitors or closing a shared connection/another receiver. After releasing a
held reply, a fresh read/reconnect and another active subscription on that connection must work.
Retain the documented zbus partial-socket-write cancellation limitation; this correction must
not silently claim arbitrary transport cancellation is safe or expand into a transport rewrite.

**PRESERVE-3 — Keep established boundaries:** Preserve QR stream draining, bounded backlog,
typed lag, receive-order retirement, watcher backoff and UI snapshot reconciliation. Preserve
authorization, pins/revocation, durable ownership and rollback, API 1.7, membership schema v3,
closed production join, disabled Internet sharing and narrow frontend permissions. No daemon,
frontend, storage, privileged-operation or wire-format feature is required.

Timer-first polling is a candidate technique, not an acceptance criterion. Review whether it
covers every resumption and stage transition and retains the post-outcome/diagnostic rules.
Use a narrowly scoped private budget-aware path or internal test seam if needed; do not expose
test hooks or add public API solely to exercise scheduling.

## 4. Regression requirements

**TEST-1 — Observable public-path regression:** Extend the maintained private-broker reconnect
suite. Match the owner's reply to the intended lookup, prove Version has not been dispatched,
pause reconnect past expiry, then require one resumed poll to return the typed timeout. Count
individual lookup, Version and Request calls before and after resumption. Require no new Version,
unchanged informational state and a healthy connection. Observe dispatch, not just the final
timeout, and distinguish construction/subscription calls from reconnect's own calls.

**TEST-2 — Boundary matrix and controls:** Cover the following alongside all existing reconnect
tests. Use acknowledgements/ordered observations to establish phases, bounded watchdogs for
fixture failure, and documented scheduler tolerance. Setup races must fail clearly rather than
silently skip the boundary. Keep brokers, fakes, temporary state and isolated controls away from
host services and physical resources.

| Scenario | Required result / evidence |
| --- | --- |
| Ready owner reply; operation resumed after expiry | Timeout on resumed poll; one initial lookup, zero reconnect Version/Request calls |
| Same owner reply resumed before expiry | Compatible success with distinct version, exactly one reconnect Version, zero Requests |
| Budget crosses inside an active poll before the next stage | Internal seam proves the next lookup/Version stage is not activated; no public test hook |
| Retry delay ends after expiry | No subsequent lookup or Version; prior observed bus diagnostic retained |
| Version was dispatched before expiry, then held or late | Existing timeout, late compatible/incompatible precedence and unchanged version still pass |
| Result/error produced after a poll began in time | Post-outcome expiry wins; available error context remains truthful |
| Zero/short/large budgets and delayed first poll | Existing zero-call, capped-delay, first-poll and overflow behavior remains correct |
| Cancellation or timeout, late reply, then fresh use | No stale state or orphan work; fresh reconnect/read and active subscription work |
| Restart/replacement or lost command reply | Existing owner binding, compatibility and no-replay controls pass |

**TEST-3 — Demonstrate sensitivity:** The corrected public regression must fail at the reviewed
baseline and pass with the fix on repeated runs. In an isolated copy, restore attempt-first
resumption without the new guard and require the no-Version assertion to fail. Separately remove
the post-outcome expiry check and require existing late-outcome tests to fail. Record exact
edits, revisions, counts and intended assertions; abort if a control patch does not apply.

## 5. Evidence, reconciliation, and closure

**EVIDENCE-1 — Exact candidate:** Execute the companion checklist's pinned Rust, explicit broker,
frontend, Python and package checks. Ensure new ignored cases actually run in CI. Record source
SHA, dirty state, toolchains, commands/results, per-suite counts/ignored cases, timing, call
counts, control results, fresh package hashes and exact-revision CI through Ralph Bridge.
The reviewed 928 workspace/94 broker/43 frontend/7 Python passes and successful CI 43942151162
are historical baselines that missed this defect; do not treat them as future acceptance or sum
overlapping workspace/experimental results.

**EVIDENCE-2 — Reconcile claims:** Create versioned implementation evidence and map every
requirement here to tasks, source and tests in `CODE_REVIEW_RECONNECT_DISPATCH_EVIDENCE.md`
when implementing this plan. Add dated corrections to the earlier reconnect
backlog/evidence and P5-005 guidance/status where needed, linking this correction as the current
RRC-01 authority. Reconcile affected parents, leaves, acceptance paragraphs and requirement rows,
especially DEADLINE-3, EVIDENCE-1 and EVIDENCE-3, without erasing historical runs or valid fixes.
Public documentation must describe both dispatch and outcome checks accurately.

**EVIDENCE-3 — Reviewed software closure:** Review the complete reconnect poll/stage path,
matrix, negative controls, typed diagnostics and cancellation limits before closing RRC-01.
Acceptance requires code, sensitive maintained tests, exact-candidate validation, CI and
consistent documentation; creating plans or passing the old suite is insufficient. Existing
QR/LC/FU/CR installed-system, physical and product/release gates stay open separately. This
software correction requires no new equipment and establishes no hardware-support claim.

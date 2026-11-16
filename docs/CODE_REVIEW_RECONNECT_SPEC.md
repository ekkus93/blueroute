# Code Review Reconnect Deadline Specification

**Status:** Proposed; no fix or acceptance is established by this document.  
**Finding:** RQR-01, P2 — `reconnect(timeout)` does not bound owner/version negotiation.  
**Reviewed baseline:** `bd58ce2355816d7c7b956b9ed7dceedb4925044a` (reviewed 2026-11-16).  
**Execution backlog:** [CODE_REVIEW_RECONNECT_TODO.md](CODE_REVIEW_RECONNECT_TODO.md).  
**Related documents:** [product specification](SPEC.md), [original backlog](TODO.md),
[queue/recovery backlog](CODE_REVIEW_QUEUE_RECOVERY_TODO.md), and
[queue/recovery evidence](CODE_REVIEW_QUEUE_RECOVERY_EVIDENCE.md).

## 1. Problem and scope

`BlueRouteClient::reconnect` in `crates/blueroute-client/src/lib.rs` computes a deadline but
awaits `validated_owner` without applying that deadline to the await. Time is checked only after
a Bus error, before a fixed 100 ms retry sleep. A compatible reply returns success without an
expiry check. Owner lookup, Version negotiation and sleep can therefore exceed the supplied budget.

The review's private-broker reproduction constructed a client with an immediate first Version
reply, then held its reconnect Version reply. `reconnect(50 ms)` was still pending after 250 ms;
releasing Version then produced `Ok(())` at approximately 251 ms. Three repeats reproduced it.
The desired result is a typed ReconnectTimeout while Version remains held, followed by rejection
of its late reply as a result of that expired operation.

This defect predates the QR changes and relates to P5-005's bounded-reconnect claim. The current
desktop/TUI shared watcher uses its own connect/backoff path, not this explicit public reconnect
method. Queue saturation, receiver release, delivery ordering and manual record recovery already
have passing QR regressions and remain preserved behavior.

The work belongs in the reusable client. It requires no daemon API, frontend workflow, transport
protocol, membership schema, role-editing API, or hardware feature change. No mutation may be
introduced or replayed to establish that a daemon has become available.

## 2. Deadline contract

**DEADLINE-1 — One operation budget:** Start one monotonic timeout budget when the reconnect
future is first polled. Use a single absolute deadline across every attempt and delay; never
restart the budget for each stage or retry. The bound applies while the caller polls the future.
If execution resumes after expiry, report expiry on that poll rather than beginning another
attempt. Timing tests must allow documented scheduler tolerance without accepting the reported
50 ms operation remaining pending at 250 ms.

**DEADLINE-2 — Bound every await:** Apply the remaining budget to current-owner lookup and
owner-bound Version negotiation, including any proxy/setup awaits within them. A held or absent
reply must not require the transport's longer timeout before reconnect can finish. Dropping the
pending read-only negotiation on expiry is acceptable; closing the shared connection to enforce
the timeout is not. A timeout wrapper must cover the full negotiation path, not just Version.

**DEADLINE-3 — Bound retry delays:** Preserve the existing retry classification, but cap each
retry sleep to the time remaining and recheck expiry before another attempt. A budget shorter
than the 100 ms poll interval must not incur a full interval. Once expired, send no new lookup
or Version attempt. Already dispatched read-only calls may finish remotely; their late replies
cannot extend or restart the expired operation.

**DEADLINE-4 — Outcome decision:** Accept success only before the deadline and update the
informational `server_version` only for that timely compatible result. An expired budget wins
when both a reply and the timeout are ready on resumption, even if the reply is compatible.
Apply the same explicit expiry precedence to a late incompatible reply; a timely incompatible
reply still fails immediately without retries. Document this rule so branch polling order cannot
silently decide the contract.

| Condition at the outcome decision | Result |
| --- | --- |
| Compatible version before deadline | `Ok(())`; update the informational server version. |
| Incompatible version before deadline | Existing IncompatibleVersion; stop without retrying. |
| Retryable bus failure with budget remaining | Retry only the permitted read-only negotiation after a capped delay. |
| Budget expired, including a pending or now-ready reply | Existing ReconnectTimeout; leave informational version unchanged. |

**DEADLINE-5 — Stable typed timeout:** Use the existing
`ClientError::ReconnectTimeout { timeout, last_error }` and Unavailable classification. Preserve
the caller's requested duration. When a prior transport error exists, retain useful diagnostic
context; when negotiation simply never replied, use a truthful coarse timeout description
rather than fabricating a bus error. Error text must not become an authorization or compatibility
decision. Do not change wire errors or expose new privileged operations.

**DEADLINE-6 — Zero and short budgets:** A zero duration returns ReconnectTimeout without
dispatching a new lookup/Version attempt or changing client state. Positive short budgets follow
the same whole-operation contract, including when the service is absent. Test zero and a budget
shorter than the retry interval explicitly; do not rely only on multi-second happy-path tests.

## 3. Compatibility, cancellation, and preserved behavior

**COMPAT-1 — Owner and command safety:** Keep compatibility negotiation bound to the resolved
unique owner, retain major/minor compatibility rules, and send zero ordinary Request calls during
reconnect. Preserve the established no-replay policy for commands that might have been delivered
or lost their replies. Timeout must not authorize delivery to an incompatible replacement.

**COMPAT-2 — State and resource lifetime:** Expiry or caller cancellation leaves the client's
informational version unchanged unless a timely success was already accepted. Late Version
replies cannot update it afterwards. Release operation-owned work without detached retry tasks,
busy polling or a leaked monitor. Keep the connection usable for subsequent reconnect/read-only
calls and other active subscriptions after a held fake reply is released. Cancellation of this
operation must not close another user's receiver or the shared bus connection.

**COMPAT-3 — Keep existing boundaries:** Preserve QR queue/backlog limits, typed lag, ordered
pre-/post-decision retirement, receiver release, watcher backoff and snapshot reconciliation.
Keep manual recovery/proof retention, authorization, identity/pins/revocation, rollback, API 1.7,
schema v3, closed production join, disabled sharing and narrow frontend permissions. Do not
rewrite the shared watcher or frontends merely to route them through this method.

## 4. Regression and evidence requirements

**EVIDENCE-1 — Sensitive isolated tests:** Convert the review recipe into maintained tests that
fail at the baseline and pass with the correction. Use the actual client and a private broker;
control Version arrival/release with acknowledgements and drop guards. A watchdog bounds fixture
failure but is not the client's timeout implementation. Keep test state/logs under ignored
workspace `tmp/`; no live system service, package installation or Bluetooth resource is required.

| Scenario | Required evidence |
| --- | --- |
| Held or never-replied Version with a short budget | Typed ReconnectTimeout within budget plus justified scheduler tolerance, while the reply remains held. |
| Reply and timer both ready after expiry | Timeout wins over late compatible/incompatible response; no informational version change. |
| Zero budget | No negotiation dispatched, immediate typed timeout, unchanged state. |
| Service absent; budget shorter than retry interval | Capped wait, bounded attempt count, no new attempt after expiry. |
| Delayed owner lookup | Whole negotiation is bounded; use a narrow internal timing seam if the real broker cannot delay this stage deterministically. Retain the real-broker held-Version test. |
| Timely compatible/incompatible replies and normal restart | Timely success or immediate incompatibility as appropriate; zero ordinary Requests and no mutation replay. |
| Caller cancellation, timeout, late reply, then fresh operation | No stale state update or orphan operation; shared connection/subscriptions remain usable. |

Use a compatible version distinguishable from the client's initial informational version when
checking late-update rejection. Record timing, requested budget, tolerance, error kind and request
counts. Run a safe negative control in an isolated checkout/target: restoring retry-only deadline
checks must fail the corrected expectations. Assertions that confirm the old bug are not passing
correctness tests. Keep test injection seams internal and narrowly scoped.

**EVIDENCE-2 — Exact candidate validation:** Run the pinned Rust, frontend, broker and package
checks listed in the companion TODO. Ensure any ignored new broker suite is explicitly scheduled
in CI. Record exact SHA, commands/results, pass/fail/ignored counts, package hashes, environment
and exact-revision CI through repository-approved tooling. Earlier 920 Rust, 84 broker and 43
frontend passes are historical baselines; they do not establish deadline correctness or fixed
future test counts. Keep mock helper/backend evidence distinct from real-system qualification.

**EVIDENCE-3 — Scope and closure:** Document the reconnect timeout semantics on the public API
and in relevant current specification/client guidance. Append a dated correction/evidence link
to P5-005's bounded-reconnect claim without discarding its working version-gating/request/event
behavior. Preserve QR/LC/FU/CR history and satisfied QR fixes; link this backlog as the current
RQR-01 authority. Map all 12 requirements to tasks/source/tests and versioned evidence.

This deadline defect has software-only acceptance. Close it when the contract, regressions,
validation, documentation and review pass. Continue reporting existing QR/LC/FU/CR installed-system,
physical and original product/release gates separately; this correction neither satisfies them
nor requires unrelated equipment before its own software acceptance. No new hardware-support
claim follows from private-broker timing results.

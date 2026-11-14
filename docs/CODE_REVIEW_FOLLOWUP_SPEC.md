# Code Review Follow-up Specification

**Status:** Proposed; this document implements or accepts no fixes.  
**Review baseline:** `6d200d0cda24acd00b1f3a6b8981b1d0bb2d6eec`, reviewed 2026-11-13.  
**Execution backlog:** [CODE_REVIEW_FOLLOWUP_TODO.md](CODE_REVIEW_FOLLOWUP_TODO.md).  
**Related documents:** [product specification](SPEC.md), [original backlog](TODO.md),
[first remediation specification](CODE_REVIEW_REMEDIATION_SPEC.md),
[first remediation backlog](CODE_REVIEW_REMEDIATION_TODO.md), and
[earlier evidence](CODE_REVIEW_REMEDIATION_EVIDENCE.md).

## 1. Purpose, authority, and scope

Close the four defects reproduced during the remediation review, repair the related startup
and qualification gaps, and reconcile claims of completion with current evidence. This file
supplements the first remediation specification. Its requirements refine EVENT, WATCH, GRAPH,
and CLEAN requirements; they do not relax them or close original P0–P19 acceptance gates.

The descriptions below are durable reproduction recipes. Acceptance must use repository tests
and versioned evidence, without depending on the review's temporary `/tmp` artifacts.

| Finding / gap | Baseline behavior and affected source | Earlier requirements / tasks |
| --- | --- | --- |
| REV-01, P2 | `apps/daemon/src/state_publisher.rs`: a pending `observable_networks()` read blocks all invalidations and worker shutdown. A published health change produced no signal for over 1.2 seconds, then signalled when the read was released. | EVENT-2/6; CR-F1-04/06/09, CR-INTEGRATION-03 |
| REV-02, P2 | `crates/blueroute-client/src/lib.rs`: ownership is no longer monitored during event-time Version validation. A relinquished owner can deliver an obsolete event or hold owner-loss reporting beyond two seconds. | OWN-4, WATCH-1/2/4; CR-F2-01/02/06/07, CR-F8-05, CR-INTEGRATION-01 |
| REV-03, P2 | `crates/blueroute-core/src/topology.rs`: new-segment insertion validates explicit members but stores cloned derived members unchanged, admitting nodes absent from the destination graph. | GRAPH-3/4; CR-F3-04/05/06, CR-INTEGRATION-05 |
| REV-04, P2 | `apps/daemon/src/removal.rs`, `client_leave.rs`: Bluetooth-unavailable cleanup removes profile/address proof, reports success with a PANU link unresolved, and leaves a later Bluetooth-enabled retry unable to disconnect it. | CLEAN-3/4/5/6; CR-F5-04/06/09, CR-INTEGRATION-06 |
| GAP-01, documented limitation | Publisher startup can absorb a change into its first baseline after a client has already read the earlier snapshot. This gap was acknowledged in earlier evidence; it is not a fifth reproduced finding. | EVENT-5; CR-F1, CR-INTEGRATION |
| GAP-02, qualification gap | `CR-QUALIFY-PROCEDURE.md` promises a Bluetooth-free Part A but creates owned state through an adapter-dependent network-create operation. | CR-QUALIFY-01..05 |

### Preserved boundaries

- Keep privileged networking in daemon/Linux adapters and shared clients. Do not add networking
  logic or broader privileges to CLI, TUI, desktop, or React hooks.
- Preserve unique-owner command delivery, mutation no-replay rules, authorization, authenticated
  NodeId/NetworkId checks, pins, revocation, rollback, and foreign-resource ownership boundaries.
- Keep production join closed, Internet sharing disabled, and the TUI read-only. Do not implement
  future forwarding, NAT, firewall, or routing orchestration to satisfy this backlog.
- Preserve the additive API 1.7 StateChanged compatibility contract and membership schema v3.
  Any necessary protocol/storage change requires an explicit compatibility/migration decision.
- Retain historical hardware evidence and separate deterministic, installed-system, and physical
  acceptance. Equipment deferral does not block independent software work.

## 2. FU-01: Keep publication independent of slow sampling

**PUB-1 — Separate progress:** Once authoritative state is committed, a pending network discovery,
storage read, or other unrelated observation must not block the emitter. Separate slow sampling
from signal delivery with bounded/coalesced state. Do not merely move the same blocking read into
another emitter callback, hold a shared lock across it, or spawn unbounded sampling workers.

**PUB-2 — Preserve dirty state:** Retain the latest invalidation until delivery or a documented
fresh-subscription reconciliation. A delayed/stale sample cannot overwrite newer observations
or clear a dirty revision created while sampling or emission was in progress. Backend failure
must not manufacture an empty successful network snapshot. Notification failure must not change
the result of an already committed operation or encourage replay.

**PUB-3 — Bound lifecycle:** Cancellation/shutdown must progress while a sampler is pending.
In the controlled pending-future regression, shutdown must finish within two seconds without
leaving an accumulating worker/task leak. Document production adapter deadlines and how blocking
work terminates; detaching an indefinitely blocked thread is insufficient cleanup.

**PUB-4 — Observable acceptance:** On a healthy private broker, hold the real network-list
backend pending after startup, then publish a real `DaemonService` health/capability change.
The connected subscriber must receive invalidation within one second of publication, before
releasing the backend. Repeat for startup sampling, sampling errors, emitter retry, and bursts.
After release/recovery, the final network state must become observable. Unchanged samples must
not produce a busy notification loop. Timing begins after state publication, not radio detection.

## 3. FU-02: Monitor ownership throughout event validation

**OWNER-1 — One lifecycle:** Observe owner loss during every await involved in consuming an
event, including compatibility validation. A pending Version reply from A must not prevent
processing a NameOwnerChanged transition retiring A. Preserve the unique-owner destination for
both Version and normal requests, typed errors, and no automatic replay of delivered mutations.

**OWNER-2 — Validate delivery:** Before accepting an event after its Version check, establish
that the checked owner is still current. Define the ownership verification/delivery decision
point and ordering against queued owner notices. If A relinquishes the name inside Version and
B acquires it before A replies, A's selected or queued event must be discarded. A check performed
only before Version does not satisfy this condition.

**OWNER-3 — Terminal retirement:** Once loss is observed, the subscription cannot deliver further
events from A, even if A remains connected and replies later. In the controlled local-broker test,
owner loss must be surfaced within two seconds of the transfer, independently of A's response.
Watchers mark retained snapshots stale/disconnected, reconnect with bounded backoff, negotiate B,
and fetch fresh snapshots. An incompatible B receives no ordinary Request calls.

**OWNER-4 — Regression scenarios:** Use barriers in fake A's event-time Version handler, rather
than sleeps that guess when validation starts. Cover compatible and incompatible B, disappearance
without replacement, A staying connected, queued events, held/late Version replies, cancellation,
and a subsequent fresh subscription. Existing command-owner/lost-reply tests must continue to pass.
Exercise the shared watcher and desktop/TUI consumers, not just a low-level proxy helper.

## 4. FU-03: Normalize or reject imported segment membership

**SEG-1 — Complete insertion contract:** Document how `add_segment` accepts cloned/imported
segments, including the distinction between explicit and link-derived membership. Prefer rebuilding
stored membership from the validated NAP, valid explicit members, and links belonging to the
destination graph. Rejecting incompatible imports is also acceptable if documented and reflected
in callers before implementation. Never trust derived members from another graph.

**SEG-2 — Whole-graph invariants:** After every successful insertion/update, all members exist
as nodes; the NAP belongs to the segment; every remaining member is explicit or justified by a
represented link in that graph; and each link has valid endpoints and the segment's NAP. An
endpoint existing in the destination graph alone does not justify imported derived membership.
Explicit non-link members remain supported according to the existing contract.

**SEG-3 — Atomicity and identity:** Validate all accepted input representations before mutation.
Invalid NAPs/explicit members or forbidden rebinding leave the entire graph unchanged. Normalize
or reject a public NAP field changed on a cloned segment. Preserve identical reinsertion,
property-only link updates, boolean return semantics, and multiple-link removal behavior.

**SEG-4 — Regression scenarios:** Clone a populated segment from graph A into graph B with
(a) a missing client and (b) the client present but no matching link. Test changed/unknown NAP,
unknown explicit member, same-graph reinsertion, and explicitly supplied membership. Assert
whole-graph consistency after success and equality after rejection; include a downstream
topology/forwarding consumer to show phantom membership cannot influence its result.

## 5. FU-04: Fail unresolved cleanup and preserve retry proof

**REMOVE-1 — Explicit observation:** Distinguish a proven absent PANU link from an unavailable
or inconclusive Bluetooth observation. For a recorded owned client attachment, no Bluetooth
connection is not evidence of absence. Success requires all applicable owned resources removed
or independently observed absent, with no unresolved required teardown. Host-only/no-client
cleanup must not gain an unnecessary Bluetooth prerequisite.

**REMOVE-2 — Proof lifetime:** Keep sufficient narrowly scoped ownership proof until the final
dependent teardown completes. Either retain the relevant profile/address evidence or define a
durable verified cleanup record with explicit schema, provenance, boot/installation validity,
and recovery rules. A host hint or interface name alone must not authorize disconnecting foreign
state. Never infer ownership from a reused interface name or clear membership to hide a failure.

**REMOVE-3 — Partial progress and retries:** Continue independent safe NetworkManager work and
other networks when Bluetooth or one resource fails. Preserve proof needed by unresolved work;
reobserve before retrying rather than replaying stale destruction. A retry with Bluetooth
restored must either disconnect the still-proven owned link or verify its absence, without
touching foreign resources. Handle process interruption between each destructive step.

**REMOVE-4 — Outcome and package lifecycle:** Include unresolved PANU work in the helper's
success predicate/report, using bounded non-secret categories and counts. Preserve the existing
60-second whole-run bound and typed/coarse failure outcomes. Nonzero cleanup must propagate
through remove/deconfigure/direct-purge paths; normal purge must not erase retry evidence after
an unresolved removal. Upgrade and abort/retry behavior must retain the established lifecycle
contract and quiesce restoration while cleanup actually runs.

**REMOVE-5 — Regression scenarios:** Start with a recorded owned client, owned profile/address,
and live PANU attachment. With Bluetooth unavailable, require failure plus preserved retry proof
while unrelated safe cleanup proceeds. Restore Bluetooth and require successful teardown, then
idempotent success. Also cover already absent links, foreign attachments, vanished or conflicting
proof, NetworkManager failure, timeout, interrupted cleanup, and empty/host-only installations.
Correct the existing fixture that accepts successful unresolved teardown. Run the production
cleanup/report path as well as maintainer lifecycle and isolated dpkg failure/retry/purge tests.

The skip-and-succeed policy in [CR-F5-REMOVAL-CLEANUP.md](CR-F5-REMOVAL-CLEANUP.md) conflicts with
CLEAN-5. Revise that design and its evidence when implementing this requirement. Whether a real
BlueZ stack already drops a link when its owner exits remains a qualification question, not a
reason to report unobserved teardown as successful.

## 6. FU-05: Close the publisher startup window

**START-1 — Initialization ordering:** Establish a publication baseline before clients can
observe an earlier state, or retain a revision/dirty handshake that forces reconciliation of
changes during initialization. Startup must not absorb an already committed observable change
into a silent baseline. This must work while initial network sampling is pending.

**START-2 — Client handoff:** Preserve subscribe-before-read or subscribe-and-reread behavior on
initial connection and each new compatible owner. A concurrent change must be included in the
snapshot or cause a subsequent invalidation; an unconditional assumption that startup is too
fast for a client to read is insufficient. No exactly-once event guarantee is required.

**START-3 — Regression evidence:** Place barriers around initial baseline capture, service
availability, snapshot read, and subscription installation. Force the formerly missed ordering
and assert eventual latest state under the one-second post-publication notification contract
when a notification is needed. Test both cold startup and replacement-owner reconnect without
manual signal injection or sleep-dependent setup.

## 7. FU-06: Make qualification executable with honest prerequisites

**QUAL-1 — Adapter-free installed-system path:** Revise Part A to provide an explicit test-only
owned NetworkManager/registry fixture usable on a disposable VM without a Bluetooth adapter.
Document exact setup, full ownership metadata and schema, state permissions, expected resources,
verification, and teardown. Use temporary dummy/bridge devices isolated from management networking.
If a fixture cannot faithfully exercise a case, move that case to Part B and state its equipment
requirement. A normal network-create operation needing NAP is not an adapter-free fixture.

**QUAL-2 — Distinct evidence layers:** State which cases prove helper/software logic, actual
installed NetworkManager/systemd/dpkg behavior, or physical PAN teardown. A seeded VM client
record with Bluetooth unavailable must produce the unresolved failure required by REMOVE-1;
it cannot be counted as successful physical client cleanup. Preserve foreign/default networking
and forwarding comparison fixtures, stopped-daemon tests, upgrades, partial failure/recovery,
remove, direct purge, and remove-then-purge coverage.

**QUAL-3 — Isolation:** Fixtures are test-only and grant no production ownership bypass. A
synthetic dpkg root relocates files, not D-Bus connections: test harnesses must prevent the real
maintenance helper reaching host services. Record whether helpers/buses are mocked, private, or
inside the disposable system. Do not describe chrootless path relocation as bus isolation.

**QUAL-4 — Deferred acceptance:** Record source SHA, package hashes, OS/kernel, NetworkManager,
systemd/dpkg/BlueZ versions, exact commands, independent before/after observations, and failures.
For physical claims, also identify computer/controller, firmware/driver and peer setup. Keep
CR-QUALIFY and affected original gates open until their actual evidence exists. Preparing this
plan authorizes no installed-system or physical execution on the user's ordinary machine.

## 8. Integration, evidence, and acceptance

**ACCEPT-1 — Useful regressions:** New tests must fail against the reviewed defective behavior
for the intended reason and pass with the fix. Use controlled pending futures/barriers and real
private-broker paths. Timeouts are assertion bounds, not orchestration. Record negative-control
results from a temporary checkout or safe defect restoration; do not leave broken tracked files.

**ACCEPT-2 — Exact candidate:** Run the checks listed in the companion TODO against the candidate
revision. Record commands, pass/fail/ignored counts, environment, package hashes, and exact-revision
CI through Ralph Bridge. Earlier green tests are historical evidence only: the review passed
888 default Rust, 31 broker, and 43 frontend tests while reproducing all four defects. Do not
freeze those numbers as future expected counts or count overlapping suites as unique coverage.

**ACCEPT-3 — Reconcile status:** Append dated corrections to the earlier backlog/evidence and
relevant product summaries. The checked CR-F1/F2/F3, integration, and software-complete CR-F5
claims require requalification. Correct affected CR-F8 coverage and CR-CLOSEOUT claims without
discarding their working behavior or historical results. Keep one linked current follow-up
status authority: the companion TODO. Each requirement above must map to a task and evidence.

**ACCEPT-4 — Separate milestones:** Software closure requires REV-01..04 and GAP-01..02 resolved,
meaningful regressions/integration checks passing, documentation consistent, and review complete.
Full cleanup/release acceptance additionally requires the existing installed-system and physical
gates. A **SOFTWARE COMPLETE / QUALIFICATION DEFERRED** milestone may be recorded while those
remain open; never label the overall remediation COMPLETE on software checks alone.

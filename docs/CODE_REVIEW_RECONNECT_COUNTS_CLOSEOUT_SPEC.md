# Code Review Reconnect Counts Closeout Specification

**Status:** No open remediation identified by the latest review.
**Reviewed source:** `25c04103875c8b9a4eadadda23a5e8446e8aea2a` (2026-11-16).
**Companion checklist:** [CODE_REVIEW_RECONNECT_COUNTS_CLOSEOUT_TODO.md](CODE_REVIEW_RECONNECT_COUNTS_CLOSEOUT_TODO.md).
**Existing task authority:** [counts backlog](CODE_REVIEW_RECONNECT_COUNTS_TODO.md).
**Versioned proof:** [dispatch evidence, RCD-REVIEW-01](CODE_REVIEW_RECONNECT_DISPATCH_EVIDENCE.md#rcd-review-01--client-test-count-correction-and-proof-2026-11-16).

## 1. Purpose and current outcome

The latest review of `CODE_REVIEW_RECONNECT_COUNTS_TODO.md` found no new actionable issues.
All ten subtasks, three parent tasks and six COUNT requirements were Verified. Its complete
inventory contained 31 Verified entries, including acceptance paragraphs, procedure groups and
requirement mappings. There is no remaining finding for a new implementation plan to fix.

These two closeout documents record that result in response to the request for a new spec and
TODO. They do not reopen accepted work, introduce synthetic defects or establish new software
requirements. Existing specifications, task IDs and evidence remain authoritative for their
respective corrections.

## 2. Previously reported issues and their disposition

| Finding | Issue | Current disposition / authority |
| --- | --- | --- |
| RQR-01 | Reconnect did not bound owner/version negotiation, late results and retry delays correctly | Corrected in the [reconnect backlog](CODE_REVIEW_RECONNECT_TODO.md), supplemented by RRC-01 |
| RRC-01 | An expired reconnect could dispatch Version when resumed with an owner reply ready | Corrected in the [dispatch backlog](CODE_REVIEW_RECONNECT_DISPATCH_TODO.md) |
| RCD-REVIEW-01 | Historical client-library tests were described as increasing 12 → 19 | Corrected and closed in the [counts backlog](CODE_REVIEW_RECONNECT_COUNTS_TODO.md) and versioned proof |

The final documentary correction uses **14 → 19** for the complete client-library harness and
**8 → 13** for its `deadline_tests` subset. Five deadline tests were added; six other library
tests remained unchanged. Library tests are already included in workspace results, and the
filtered subset is already included in the library total. The separate 12-case private-broker
suite was recorded correctly and was not changed to repair the unit-test comparison.

## 3. Evidence supporting closure

The preceding review independently ran both locked harness-listing commands at `a7abc60`,
`0257bcb`, `a1702ad` and `3a9a631`, and at reviewed HEAD. It checked exact source equivalence,
complete discovered names, totals, the five-test addition and unchanged six-test remainder.
Listings establish test discovery; they do not execute assertions.

The corrected evidence contains immutable SHAs, commands, names, totals, dated corrections and
closure references. Related RCD-VALIDATE-05, RCD-CLOSEOUT-04/05, parent acceptance and
EVIDENCE-1/2/3 claims were reconciled. Valid historical runtime results, package hashes and CI
identities were preserved at their original revisions.

The preceding review also confirmed successful exact-revision
[CI 43974032161](https://github.com/ekkus93/blueroute/actions/runs/43974032161), including Rust and
Desktop frontend jobs. This closeout record does not represent a new CI run or a new execution
of runtime, package or hardware tests.

## 4. Scope and limits

Runtime code, tests and build configuration were unchanged by the counts correction. Broader
unchanged subsystems retain the preceding sampled-review coverage; the absence of new findings
does not assert an exhaustive audit or that all product functionality is complete.

The documented zbus partial-write cancellation limitation and existing installed-system,
physical, product and release gates remain separate. This record neither resolves nor expands
those limits, and makes no new hardware-support or transport-safety claim.

## 5. Acceptance and any later work

Acceptance for these closeout documents means that their statements agree with the accepted
counts backlog and versioned proof, local links resolve, historical attribution is preserved,
and no unsupported pending remediation is introduced. The companion TODO therefore records
zero open corrective tasks.

A later actionable finding should identify its source revision, location, triggering condition,
impact and evidence before receiving a scoped task and acceptance criteria. Unrelated planned
features and existing qualification work continue in their own backlogs. No new review or test
campaign is required merely because this closeout document exists.

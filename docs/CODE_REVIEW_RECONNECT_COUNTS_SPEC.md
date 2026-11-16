# Code Review Reconnect Evidence Counts Specification

**Status:** Proposed; no correction or acceptance is established by this plan.  
**Finding:** RCD-REVIEW-01 / P3 — inaccurate historical client test-count attribution.  
**Reviewed revision:** `3a9a63114d369dd1eec1702f143272ac8d704aa9` (2026-11-16).  
**Execution backlog:** [CODE_REVIEW_RECONNECT_COUNTS_TODO.md](CODE_REVIEW_RECONNECT_COUNTS_TODO.md).  
**Affected record:** [dispatch evidence](CODE_REVIEW_RECONNECT_DISPATCH_EVIDENCE.md),
“RCD-VALIDATE — Exact candidate.” Related: [dispatch backlog](CODE_REVIEW_RECONNECT_DISPATCH_TODO.md),
[dispatch specification](CODE_REVIEW_RECONNECT_DISPATCH_SPEC.md),
[earlier reconnect evidence](CODE_REVIEW_RECONNECT_EVIDENCE.md), and [original backlog](TODO.md).

## 1. Finding and scope

The dispatch evidence correctly reports workspace tests increasing from 928 to 933, but its
explanation says the client unit count increased from 12 to 19. The stated baseline,
`a7abc60`/`0257bcb`, actually has 14 client-library unit tests. The dispatch candidate,
`a1702ad`, and reviewed revision `3a9a631` have 19: five additional deadline tests.

The review independently confirmed the baseline/current harness listings. This is an evidence
accuracy issue, with no demonstrated runtime impact. RRC-01's dispatch guards, regression
sensitivity, current workspace total and runtime acceptance remain supported. The review marked
RCD-VALIDATE-05 and RCD-CLOSEOUT-04/05 Partial solely for this documentary correction; related
parent acceptance and EVIDENCE-1/2/3 mappings inherit the same gap, not additional code defects.

Implementation consists of correcting the count, recording reproducible provenance, and
reconciling those completion claims. No Rust, frontend, protocol, dependency, packaging or
configuration change is required. Preserve existing hardware and installed-system gates,
security boundaries and the documented zbus cancellation limitation.

## 2. Correct comparison and terminology

| Scope | Baseline `a7abc60` / `0257bcb` | Candidate `a1702ad` / review `3a9a631` | Change |
| --- | ---: | ---: | ---: |
| Entire `blueroute-client` library unit-test harness | 14 | 19 | +5 |
| Tests selected by `deadline_tests` within that harness | 8 | 13 | +5 |
| Other client-library tests | 6 | 6 | 0 |
| Workspace passed tests, historical recorded runs | 928 | 933 | +5 |

The full library harness includes tests in both `src/lib.rs` and `src/watch.rs`. Avoid calling
the crate-wide count an “in-file” count. `deadline_tests` is a subset, not another independent
suite to add to the library total. Likewise, library tests are already included in workspace
results. The 12 private-broker `reconnect_deadline` cases are a different integration suite;
their recorded count is correct and must not be changed to repair the unit-test comparison.

## 3. Required correction

**COUNT-1 — Exact suite attribution:** Correct the affected parenthetical to state that
client-library tests rose **14 → 19 (+5)**. If filtered deadline tests are also discussed, state
**8 → 13 (+5)** and identify the filter. Clarify ambiguous “seam” labels where they could imply
that all 19 library tests belong to `deadline_tests`. Keep comparisons tied to their revisions;
these are not fixed expectations for future codebases.

**COUNT-2 — Reproducible provenance:** Record resolved source SHAs, Rust toolchain, exact commands,
test names and listing totals for the compared revisions. Historical listings must use the
historical source, including its watch module, rather than a current checkout relabeled as the
baseline. Use ignored isolated checkouts if needed. A listing proves discovery/counts, not that
the tests passed. Attribute execution results only to actual recorded runs.

**COUNT-3 — Preserve valid results:** Keep the correct workspace totals, broker results, package
hashes, CI identities, negative controls and runtime conclusions attached to their original
revisions. Correct the inaccurate text and add a dated explanation in the existing dispatch
evidence. Preserve genuinely valid older counts from other revisions; do not globally replace
every occurrence of 12 or 19, rewrite the earlier baseline, or sum overlapping suites.

**COUNT-4 — Reconcile acceptance:** Add a dated correction/closure reference to the dispatch
backlog and evidence for RCD-VALIDATE-05, RCD-CLOSEOUT-04/05 and affected parent/mapping claims.
Explain that the remaining gap was evidence accuracy, and close it only after the corrected
comparison is verified. Preserve stable task IDs, earlier review history and working RRC-01
acceptance. This counts backlog is the authority for RCD-REVIEW-01, not a new runtime backlog.

**COUNT-5 — Proportionate validation:** Check Markdown links/anchors, suite labels, revision
identity, count arithmetic, requirement coverage and the final change scope. Review listing
output rather than relying solely on a source-text count of test attributes. Follow required
repository submission checks; this correction introduces no separate full runtime, package,
negative-control or physical requalification campaign. If source behavior changes during the
work, reassess scope and run checks appropriate to that change.

**COUNT-6 — Evidence and closure:** Store the count proof and correction in the existing
versioned dispatch evidence; no temporary review artifact may be the sole acceptance record.
Map these six requirements to checklist tasks and final evidence. Close RCD-REVIEW-01 after the
documentation and proof agree. Keep new checklist items pending until their own criteria pass;
creating this plan does not correct the existing record or accept implementation work.

## 4. Verification procedure

With Rust 1.95.0, run these commands against each explicitly identified source revision:

```bash
cargo test -p blueroute-client --lib --locked -- --list
cargo test -p blueroute-client --lib --locked deadline_tests -- --list
```

The first listing must show 14 baseline and 19 candidate tests; the filtered listing must show
8 and 13 respectively. Capture test names, not only the summary line. Confirm that the five
added names are the RCD deadline regressions and the other six library tests remain unchanged.
Verify source equivalence before using `0257bcb` as the `a7abc60` equivalent or `3a9a631` as the
`a1702ad` equivalent. Later revisions may have additional tests and require separate attribution.

Retain enough command output or a compact complete names/count table in the existing evidence
to reproduce the conclusion without ignored files. Keep listing results distinct from the
previously recorded 933 workspace passes and 96 explicitly executed broker passes. If CI is
cited for the correction, identify the actual commit/run; do not relabel historical CI as a
fresh documentation run.

## 5. Acceptance

The erroneous 12-to-19 comparison is corrected, the library/filter distinction is clear, and
immutable-revision proof supports the replacement. A dated note reconciles the three affected
subtasks, parent acceptance and EVIDENCE-1/2/3 mappings without weakening prior criteria or
reopening verified runtime work. Links resolve, all six requirements have evidence, and the
implementation diff is limited to the planned documentation. Physical qualification remains
separate and no new hardware-support claim is made.

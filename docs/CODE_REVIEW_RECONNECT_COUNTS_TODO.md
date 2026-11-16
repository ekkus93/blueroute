# Code Review Reconnect Evidence Counts TODO

**Status:** IN PROGRESS; RCN-BASE accepted.  
**Finding / priority:** RCD-REVIEW-01 / P3.  
**Reviewed revision:** `3a9a63114d369dd1eec1702f143272ac8d704aa9`.  
**Companion specification:** [CODE_REVIEW_RECONNECT_COUNTS_SPEC.md](CODE_REVIEW_RECONNECT_COUNTS_SPEC.md).  
**Implementation evidence destination:** a dated correction and count-proof section in
[CODE_REVIEW_RECONNECT_DISPATCH_EVIDENCE.md](CODE_REVIEW_RECONNECT_DISPATCH_EVIDENCE.md).  
**Related work:** [dispatch backlog](CODE_REVIEW_RECONNECT_DISPATCH_TODO.md),
[dispatch specification](CODE_REVIEW_RECONNECT_DISPATCH_SPEC.md), and [original backlog](TODO.md).

## Tracking and boundaries

- Use stable `RCN-*` IDs and reference RCD-REVIEW-01 and the affected `RCD-*` tasks in evidence.
  This register governs the evidence-count correction only.
- `[ ]` pending, `[-]` in progress, `[x]` accepted, `[!]` blocked with a reason. Keep parents,
  children and acceptance notes consistent; do not check items merely because a plan exists.
- Preserve the verified RRC-01 code and tests, historical results, source SHAs, package hashes,
  CI runs, security boundaries and existing physical/installed-system/release gates.
- Correct documentation and its supporting attribution. No new implementation feature, transport
  safety claim, broad test campaign or equipment dependency follows from this P3 finding.
- Use versioned proof that survives cleanup of ignored review artifacts. Listings establish
  discovered tests, while passing assertions require actual execution evidence.

## Task register and dependencies

| Task | Outcome | Dependencies | State |
| --- | --- | --- | --- |
| RCN-BASE | Confirm exact revisions, suite names and count proof | None | `[x]` |
| RCN-CORRECT | Correct the evidence and reconcile affected claims | RCN-BASE | `[ ]` |
| RCN-CLOSEOUT | Validate documents, complete traceability and close the correction | RCN-CORRECT | `[ ]` |

## RCN-BASE — Establish the count comparison

**Requirements:** COUNT-1/2/3.

- [x] **RCN-BASE-01:** Record HEAD, dirty state, pinned Rust toolchain and differences from
  reviewed `3a9a631`. Read the finding, companion spec and affected evidence/backlog. Identify
  the incorrect parenthetical in “RCD-VALIDATE — Exact candidate”; distinguish it from the
  correctly recorded workspace, broker and current client totals.
- [x] **RCN-BASE-02:** Resolve baseline `a7abc60`/`0257bcb` and candidate `a1702ad`/`3a9a631`
  to immutable SHAs. Confirm client source equivalence within each pair before using an
  equivalent revision. Obtain full-library and filtered `deadline_tests` harness listings from
  the correct sources, including `watch.rs`, using ignored isolated checkouts if needed.
- [x] **RCN-BASE-03:** Record complete names/count proof: library 14 → 19, deadline subset
  8 → 13, six other library tests unchanged, five RCD deadline tests added. Explain that library
  and filtered counts overlap and listing does not execute tests. Keep the evidence independent
  of machine-specific temporary paths or prior review scripts.

**Acceptance:** Exact revision and harness evidence supports both comparisons and the +5 delta.
The baseline is not inferred from a current listing or a count of attributes in one source file.

## RCN-CORRECT — Repair the evidence and status attribution

**Requirements:** COUNT-1/3/4/6.

- [ ] **RCN-CORRECT-01:** Replace the incorrect 12-to-19 parenthetical with the verified
  14-to-19 client-library comparison. Remove the misleading “in-file” label; identify 8-to-13
  filtered deadline tests when useful. Check nearby “seam” terminology and qualify whether it
  means all library tests or only `deadline_tests`, without inventing unrecorded past commands.
- [ ] **RCN-CORRECT-02:** Add a dated RCD-REVIEW-01 correction and reproducible count-proof
  section to the existing dispatch evidence. Include resolved SHAs, toolchain, commands and
  names/counts. Preserve valid results at their original revisions, including workspace 928/933,
  the 12-case broker suite, package hashes, CI and controls; do not perform a blanket replacement.
- [ ] **RCN-CORRECT-03:** Reconcile RCD-VALIDATE-05 and RCD-CLOSEOUT-04/05, their parent
  acceptance and EVIDENCE-1/2/3 mapping claims through dated backlog/evidence references to this
  correction. State that the three Partial review verdicts share one documentation root cause;
  retain stable IDs/history and verified runtime acceptance. Do not reopen unrelated code or
  hardware gates, or imply the original dispatch correction was unimplemented.

**Acceptance:** The inaccurate count is corrected with a traceable explanation, related status
claims are consistent, and all previously valid behavior/results retain their original meaning.

## RCN-CLOSEOUT — Verify the correction and finish the ledger

**Requirements:** COUNT-4/5/6.

- [ ] **RCN-CLOSEOUT-01:** Independently compare the corrected prose with the harness outputs
  and immutable source revisions. Verify suite names, the 14/19 and 8/13 totals, +5 arithmetic,
  unchanged six-test remainder and separation from workspace/broker counts. Correct conflicting
  local summaries of this same comparison if any are found; preserve valid older comparisons.
- [ ] **RCN-CLOSEOUT-02:** Check Markdown file/anchor links, task IDs, parent/child states and
  requirement mappings. Inspect the final diff for documentation-only scope. Follow applicable
  repository submission checks and accurately record any CI run cited for the correction;
  document why no additional runtime/package/hardware campaign is needed for this text change.
- [ ] **RCN-CLOSEOUT-03:** Complete the six-row requirement ledger below with exact source
  revision, corrected section, commands/results and dated acceptance reference. Preserve the
  distinction between new count verification and historical passing runtime tests. Resolve
  review discrepancies before marking children or parents accepted.
- [ ] **RCN-CLOSEOUT-04:** Close RCD-REVIEW-01 and this documentation correction only after
  all criteria pass. Link the accepted correction from the affected RCD evidence/backlog and
  reconcile its documentary completion claims. Report existing installed-system, physical and
  release gates separately; make no new transport-safety or hardware-support claim.

**Acceptance:** Corrected documentation, immutable-revision proof, task states and all six
requirement mappings agree. The correction is reviewable without the original temporary report;
verified runtime work and separate qualification gates remain accurately attributed.

## Verification commands

Use Rust 1.95.0 and run in each explicitly identified baseline/candidate checkout:

```bash
cargo test -p blueroute-client --lib --locked -- --list
cargo test -p blueroute-client --lib --locked deadline_tests -- --list
```

Record the source SHA and toolchain alongside each listing:

```bash
git rev-parse HEAD
git status --short
rustc --version
cargo --version
```

Keep historical checkout/target scratch under ignored workspace `tmp/`; do not switch the main
checkout away from the user's work to count an older revision. Capture stdout/stderr and command
exit status. Use harness totals and names as the count authority, not a source-regex estimate.
The listings may compile tests but do not execute their assertions or ignored broker scenarios.

If unrelated changes make the working tree's counts differ, record them separately and preserve
the immutable comparisons above. Required submission checks still apply; this plan adds no full
workspace/frontend/package rerun solely to substantiate a corrected historical count.

## Requirement and evidence ledger

Populate during correction; all rows are pending at plan creation.

| Requirement | Tasks | Required acceptance evidence | State |
| --- | --- | --- | --- |
| COUNT-1 | RCN-BASE-03, RCN-CORRECT-01, RCN-CLOSEOUT-01 | Correct 14 → 19 library and 8 → 13 filtered labels | PENDING |
| COUNT-2 | RCN-BASE-01/02/03 | Immutable revisions, exact commands, names and listing totals | PENDING |
| COUNT-3 | RCN-BASE-01, RCN-CORRECT-02, RCN-CLOSEOUT-01 | Dated correction preserving valid historical results and suite boundaries | PENDING |
| COUNT-4 | RCN-CORRECT-03, RCN-CLOSEOUT-01/04 | Affected RCD leaves, parents and EVIDENCE mappings reconciled | PENDING |
| COUNT-5 | RCN-CLOSEOUT-01/02 | Accurate arithmetic/labels, valid links, scoped diff and required checks | PENDING |
| COUNT-6 | RCN-CORRECT-02, RCN-CLOSEOUT-03/04 | Complete versioned proof and reviewed documentation closure | PENDING |

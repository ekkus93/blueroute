# Code Review Reconnect Dispatch Evidence

Versioned evidence for [CODE_REVIEW_RECONNECT_DISPATCH_TODO.md](CODE_REVIEW_RECONNECT_DISPATCH_TODO.md)
(specification: [CODE_REVIEW_RECONNECT_DISPATCH_SPEC.md](CODE_REVIEW_RECONNECT_DISPATCH_SPEC.md)).
Software results only, from private session brokers; nothing here is installed-system or physical
qualification, and no hardware-support claim follows from these timings.

## RCD-BASE — Finding preserved and corrected expectation established (2026-11-16)

**Environment (RCD-BASE-01).** Source `0257bcbf11f37276771a477e612529fc3e10f155`, the reviewed
baseline, plus the two RRC-01 plan documents (`8ca0ad3`); no code difference. Clean tracked tree
apart from the never-committed `.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, Node 26.13.0, npm
11.20.0, Python 3.13.5, dbus-daemon 1.16.2, zbus 5.19.0 (async-io), Debian 13 kernel 6.12.115.
Private brokers via `dbus-run-session`; temporary state under the ignored workspace `tmp/`.
Guidance read: `CLAUDE.md`, `docs/SPEC.md` client paragraph, `docs/TODO.md` and
`docs/P5-005-CLIENT.md` P5-005, the RQR-01 reconnect specification, backlog and evidence, and this
specification and backlog.

**Separation from RQR-01.** RQR-01 (unbounded negotiation, late success, uncapped delay, zero
budget) stays fixed: all its regressions pass at the baseline. RRC-01 is a remaining dispatch
boundary in that fix, not a regression of it.

### RRC-01 — an expired reconnect can send a new Version call (RCD-BASE-02)

- **Affected code** (`crates/blueroute-client/src/lib.rs` at `0257bcb`, `negotiate_within`):

  ```rust
  futures_lite::future::or(async { Some(attempt().await) }, async {
      Timer::at(deadline).await;
      None
  })
  ```

  `future::or` polls its first future first. When the caller resumes a reconnect after its
  deadline while the owner lookup's reply is ready, that poll completes `current_owner` inside
  `validated_owner` and continues into `negotiate_version_at`, which builds the owner proxy and
  sends Version before the timer branch is polled. The timer (or the post-outcome expiry check)
  then returns `ReconnectTimeout`. The result and state stay correct; the extra read-only
  dispatch violates DEADLINE-3 and the documented "nothing new is sent after it".
- **Why RQR-01's tests missed it.** The late-reply broker tests pause only after Version was
  dispatched; the delayed-lookup seam test holds the lookup pending while the operation keeps
  being polled, so no attempt is ever resumed after expiry with a ready stage result.
- **Review reproduction (historical).** Public `reconnect(50 ms)` on a private broker: lookup
  dispatched, its reply ready, the future left unpolled; daemon Version count 1 before resuming
  at about 110.5 ms; one poll returned the typed 50 ms timeout and the count became 2. Three
  runs failed the corrected assertion; the timely-owner control passed three times. The review's
  ~161 ms final observation included 50 ms after the result, not reconnect's return latency. An
  isolated timer-first copy passed both probes; that was a diagnosis, not a fix.

### Corrected expectation (RCD-BASE-03/04)

Added to `crates/blueroute-client/tests/reconnect_deadline.rs`:

- `rcd_an_expired_resumption_with_a_ready_owner_reply_sends_no_version` (the regression) and
  `rcd_a_timely_resumption_with_a_ready_owner_reply_negotiates_once` (the control).
- Phases are established by observation, not timing. A bus monitor (`Dispatches`) sees every
  method call the client sends, in broker routing order, and counts lookups (by serial), Version
  and Request calls individually. The helper `owner_reply_ready_unpolled` polls the reconnect
  once, then waits **unpolled** on the monitor for its `GetNameOwner`, polling again only if none
  was sent, so the operation is never polled while the reply may be available. It then waits on
  a client-connection `MessageStream` for the method return whose reply serial matches that
  lookup (zbus 5.19 hands a reply to its pending call before broadcasting it to streams), and
  requires zero Version calls so far. Any phase not reached fails the test with its own message.
- The regression resumes at `first poll + 100 ms` (twice the budget), requires the one resumed
  poll to return `ReconnectTimeout(50 ms)`, then a broker round trip on the client connection
  (everything sent before it has been routed) and a 100 ms quiet monitor window, and requires
  exactly (1 lookup, 0 Version, 0 Request) from reconnect, 1 daemon Version call (construction's),
  an unchanged `server_version` and a working fresh reconnect. Construction's Version precedes
  the monitor and is counted separately by the daemon.
- The control resumes in time (2 s budget) and requires success with the distinct 1.8, exactly
  (1, 1, 0) and 2 daemon Version calls.
- Fixture safety as for the rest of the suite: watchdogs bound every wait, scenarios serialize on
  one mutex and release the name even on panic.

Baseline, three repeats (`dbus-run-session -- cargo test -p blueroute-client --test
reconnect_deadline --locked -- --ignored rcd_`), identical each time: the regression **FAILED**
`reconnect sent (lookups, Version, Request) = (1, 1, 0) after expiry; expected (1, 0, 0)` (owner
reply ready at 0.36 / 0.30 / 0.21 ms, resumed at 100.09 / 100.11 / 100.11 ms, result on that
poll, daemon Version count 1 before resuming); the control passed.

## RCD-GUARD — Expired work cannot advance into a new dispatch (2026-11-16)

**Source.** `3fd226a` (`fix(RCD-GUARD)`), `crates/blueroute-client/src/lib.rs`. All changes are
private to reconnect's path; no public API, wire, daemon or frontend change.

**Paths reviewed (RCD-GUARD-01).** `reconnect` → `negotiate_within` → one attempt per try.
Before: the attempt was `validated_owner`, whose awaits are `current_owner` (`GetNameOwner`
`call_method`, the lookup dispatch), then `negotiate_version_at` → `owner_proxy`
(`Proxy::new_owned`, lazy property caching: no I/O, no task) → `proxy.call("Version")` (the
Version dispatch). `validated_owner` is also used by every ordinary request and by subscriptions;
those callers have no budget and keep it unchanged.

**Decision points.**

| Point | Where | Rule |
| --- | --- | --- |
| Budget start | `Budget::starting_now`, first statement of `negotiate_within`'s async body | taken on first poll, never restarted; `checked_add` overflow means "never expires" (DISPATCH-1) |
| New attempt | loop head of `negotiate_within` | expired: typed timeout, nothing started (zero budget, end of a capped retry delay) |
| Resumed poll | `poll_fn` in `negotiate_within`: `budget.expired()` before `negotiation.poll` | expired: `Ready(None)`; the attempt is not polled onward, so a stage result that became ready while the caller was away is never consumed (DISPATCH-2). The deadline timer is polled only after a pending attempt, purely to wake the task at the deadline |
| Stage transition | `in_stages`: `budget.admit()` before the lookup and again before the Version stage (proxy setup + call) | expired: `Halted::Expired`, the next stage is not started, even inside a poll that began in time (DISPATCH-3) |
| Outcome | `negotiate_within` after each attempt: `budget.expired()` before the outcome is examined | expired: typed timeout whatever the outcome (DISPATCH-4) |

A call admitted before the deadline may still be delivered and answered afterwards; that
answer loses at the outcome check. Wall-clock time passing while the caller does not poll needs
no remote cancellation, and no claim is made about OS scheduling or socket delivery after a valid
admission.

**Diagnostics (DISPATCH-5).** Same `ReconnectTimeout { timeout, last_error }`, Unavailable.
`last_error` by case: the observed bus failure itself (expiry found at the loop head after a
retry delay); "the budget ran out before the daemon was contacted" (zero budget); "… was still
pending at the deadline" (resumed-poll or timer expiry; the pending work is not polled for better
text); "the deadline passed before the next owner lookup or Version call" (stage admission
refused); "the daemon answered only after the deadline" (late success); "the last attempt failed
/ ended only after the deadline: {error}" (late bus failure / other late error, e.g. late
incompatibility). Each appends "; the previous attempt failed: {failure}" when an earlier bus
failure was observed.

**Preserved (RCD-GUARD-04..06).** `server_version` assigned only after `Ok`; retry only on
`ClientError::Bus`; timely incompatibility and other typed errors return at once; capped delays;
no spawned task, no connection close; zbus mid-write cancellation limitation still documented.
Public rustdoc on `reconnect` now states both the dispatch and the outcome checks.

## RCD-TEST — Resumption, stage-transition and preserved-behavior matrix (2026-11-16)

New tests: broker `rcd_an_expired_resumption_with_a_ready_owner_reply_sends_no_version`,
`rcd_a_timely_resumption_with_a_ready_owner_reply_negotiates_once` (RCD-BASE); seam (`deadline_tests`)
`rcd_an_expired_resumption_does_not_poll_the_attempt_onward` (an attempt with no stage guard, so
the resumed-poll check is isolated), `rcd_a_timely_resumption_negotiates_once`,
`rcd_expiry_inside_a_poll_does_not_start_the_next_stage` (through the production `in_stages`),
`rcd_a_retry_delay_ending_at_expiry_starts_no_lookup`, `rcd_a_stage_started_in_time_that_decides_late_loses`
(late 1.8, late 2.0, late bus failure). One existing seam assertion changed wording only
("did not answer" → "still pending").

| Scenario (spec TEST-2) | Tests | Result at `3fd226a` |
| --- | --- | --- |
| Ready owner reply, resumed after expiry | broker `rcd_an_expired_resumption…`; seam `rcd_an_expired_resumption_does_not_poll…` | one resumed poll → `ReconnectTimeout(50 ms)`; monitor (1 lookup, 0 Version, 0 Request); daemon Version count 1 (construction); version 1.7; fresh reconnect → 1.8. Owner reply ready at 0.28 / 0.23 / 0.30 ms, resumed at 100.08 / 100.12 / 100.10 ms |
| Same, resumed in time | broker `rcd_a_timely_resumption…`; seam `rcd_a_timely_resumption_negotiates_once` | success 1.8; monitor (1, 1, 0); daemon count 2; seam 1 negotiation |
| Expiry inside an active poll | seam `rcd_expiry_inside_a_poll…` | lookup 1, Version stage 0; "before the next owner lookup or Version" |
| Retry delay ends after expiry | seam `rcd_a_retry_delay…`, `rc_retry_delays…`; broker `rc_an_absent_service…` | 1 lookup; `NameHasNoOwner` kept; 20 ms budget in 20.08–20.10 ms |
| Version dispatched before expiry, held or late | broker `rc_a_held_version…`, `rc_expiry_wins_over_a_late_{compatible,incompatible}_reply` | timeout in 50.05–50.06 ms; late replies lose; version unchanged |
| Result produced after a timely-started poll | seam `rcd_a_stage_started_in_time…`, `rc_an_outcome_decided_after…`, `rc_a_failure_after_the_deadline…` | timeout; truthful text per outcome kind |
| Zero / short / large budgets, delayed first poll | `rc_a_zero_budget…` (both), `rc_an_unrepresentable_budget…`, `rc_the_budget_starts_at_the_first_poll`, broker unpolled-future check | unchanged, all pass |
| Cancellation, late reply, fresh use | broker `rc_a_cancelled_reconnect…` | unchanged, passes |
| Restart, incompatible replacement, lost reply | broker `rc_a_restarted…`, `rc_a_timely_incompatible…`, `rc_a_command_whose_reply_was_lost…`, `dbus_client` | unchanged, pass |
| QR queues, owner binding, watchers | `owner_race` 6, `event_owner_race` 6, `ownership_period` 12, `queue_saturation` 18, `owner_watch` 3, `watcher` 8; desktop/TUI watcher suites in RCD-VALIDATE | pass |

Seam 19/19; broker `reconnect_deadline` 12/12 on three repeats at `3fd226a`.

### Negative controls (RCD-TEST-07)

`tmp/rcd-neg-controls.sh 3fd226a` (git-ignored helper; edits recorded here): a detached worktree
with its own `CARGO_TARGET_DIR`; each control applies one Python edit to `lib.rs` (asserting its
anchor, and reporting "CONTROL NOT APPLIED" instead of running unmodified code), runs
`deadline_tests` and the broker suite, and resets.

| Control | Edit | Seam FAILED | Broker FAILED |
| --- | --- | --- | --- |
| Y1 the `0257bcb` dispatch | remove the resumed-poll check and make `admit` always allow | `rcd_an_expired_resumption_does_not_poll…`, `rcd_expiry_inside_a_poll…` | `rcd_an_expired_resumption_with_a_ready_owner_reply_sends_no_version` |
| Y2 no resumed-poll check | remove only the `poll_fn` check | `rcd_an_expired_resumption_does_not_poll…` | none: stage admission alone still stops the Version |
| Y3 no stage admission | make only `admit` always allow | `rcd_expiry_inside_a_poll…` | none: the resumed-poll check alone still stops it |
| Y4 no post-outcome expiry check | check expiry after the attempt only when it did not finish | `rc_an_outcome_decided_after…`, `rc_a_failure_after_the_deadline…`, `rcd_a_stage_started_in_time…`, `rcd_expiry_inside_a_poll…` | none (see below) |

The guards are deliberately redundant on the broker path: each is caught on its own by a seam test
built to isolate it, and the broker regression fails when both are gone (the reviewed bug). Under
Y4 the broker late-reply tests now pass because they resume after expiry, where the resumed-poll
check decides first; an outcome decided inside a poll that began in time needs an executor
blocked across the deadline, which only the seam tests can arrange. The existing RQR-01
late-outcome seam test (`rc_an_outcome_decided_after_the_deadline_loses_to_it`) is the one that
fails for Y4, as TEST-3 requires.

## RCD-CLOSEOUT-02 — Independent review (2026-11-16)

An independent reviewer read the specification and the code at `3fd226a`/`7ad46e1`/`08e50fa`
(not this record's conclusions), checked zbus 5.19 internals and ran the suites (including 110
broker runs sequentially, under CPU saturation and with six suites concurrently).

**No dispatch bug found.** Resumed polls check expiry before polling the attempt; `in_stages`
admits before the lookup and before Version; the loop head checks before every new attempt.
`Proxy::new_owned` does no I/O (`CacheProperties::Lazily`); `call_method_raw` sends on its first
poll and a later poll that would finish a delayed send is itself guarded; the post-outcome check is
kept. The only window is the microseconds between `admit` and the send inside one poll that began
in time, which DISPATCH-3 accepts and the rustdoc states. `poll_fn` always registers a waker
(attempt, plus timer when there is a deadline), the Timer is never re-polled after firing and its
rule agrees with `expired()`; `validated_owner` for requests/subscriptions is byte-identical;
nothing is spawned; `server_version` is set only on `Ok`.

| # | Finding | Resolution (`4df1339`) |
| --- | --- | --- |
| 1 | Test gap: the fixture's "no Version yet" check could not fail (the monitor was read only up to the lookup), and the helper re-polled after 20 ms, which could let a poll find the reply and send Version inside the budget, making the timely control pass without testing the boundary | The helper polls exactly once (fails clearly if no lookup appears); after the matched reply it takes a round trip and a quiet monitor window and asserts (lookup, Version, Request) and daemon counts as deltas before resuming; the regression compares full before/after counts |
| 2 | Possible load flake (one unexplained failure during a concurrent validation) | Reproduced under 16 busy loops: 2 of 10 runs, "before resuming, reconnect had sent (1, 1, 0)". Cause: the client's reader thread delivered the lookup reply while the single first poll was still running, so that poll legitimately went on to Version. The fixture now reports such a setup as `Raced`, discards the operation and retries with a fresh one (at most 20 times, counted and printed); these two tests use a 200 ms budget so setup finishes well inside it |
| 3 | Nit: the unreachable `Halted::Expired`/`None` arm dropped the observed bus failure | It now appends the previous failure like the other arms |
| 4 | `versions_before` only printed | Asserted (finding 1) |

**After the fixes** (`4df1339`): seam 19/19; broker `reconnect_deadline` 12/12 on 10 idle runs and
15 runs under 16 busy loops (3 of those discarded one raced setup each and then tested the
boundary). Against the `0257bcb` client with the reworked test, three runs: the regression
FAILED each time with `before Counts { sent: (1, 0, 0), daemon_versions: 1 }, after Counts {
sent: (1, 1, 0), daemon_versions: 2 }`; the timely control passed. Controls Y1–Y4 rerun at
`4df1339`: identical to the table above.

## RCD-VALIDATE — Exact candidate (2026-11-16)

**Candidate** `a1702ad63ee1325fdfb3a945b0d63075efa8a06a` (code identical to `4df1339`; `a1702ad`
adds only documentation). Clean tracked tree apart from `.serena/` and `AGENTS.md`. rustc/cargo
1.95.0, Node 26.13.0, npm 11.20.0, Python 3.13.5, dbus-daemon 1.16.2, Debian 13 kernel
6.12.115. Steps are the backlog's command reference plus every `ci.yml` broker suite (run by the
git-ignored helper `tmp/validate.sh`; the commands are the ones listed in the TODO).

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check`; locked build, check; strict all-target Clippy | all rc=0 |
| `cargo test --workspace --locked` | 933 passed, 0 failed, 97 ignored (the broker cases, run below) |
| `cargo test -p blueroute-daemon --features experimental-join --locked`; its Clippy | 392 passed, 0 failed, 16 ignored; clean |
| Default build keeps join closed | 61 join tests pass, including `default_builds_cannot_open_the_join_gate` |
| Private brokers (`dbus-run-session`) | `dbus_service` 1, `authorization` 1, `dbus_client` 1, `cli_exit_contract` 1, `bridge_end_to_end` 1, `owner_race` 6, `event_owner_race` 6, `ownership_period` 12, `queue_saturation` 18, **`reconnect_deadline` 12**, `owner_watch` 3, `watcher` 8, `state_events` 8, `publisher_progress` 6, `watch_integration` 2, `watch_handover` 6, `tui_watcher` 4; all passed |
| Python socket tooling | 7 passed |
| Frontend (`npm ci`, format, lint, typecheck, test, build) | all rc=0; 4 files, 43 tests (component and StrictMode) passed; no frontend or Tauri file changed since `0257bcb`, so permissions and notifications are as before |
| Debian packages | `blueroute_0.1.0_amd64.deb` `f6ce1200e6f50c4ea95e82305e3abe226f2144da4505e2eca902cba731f9c60c` (19 entries; Depends dbus, bluez, network-manager, polkitd, systemd); `blueroute-desktop_0.1.0_amd64.deb` `abeb7f740bcf370383d8fb6320f0e9f57d4238601d4ecac2e45127fcd32a29ae` (12 entries; Depends blueroute (= 0.1.0) and the GTK/WebKit runtime) |
| Maintainer lifecycle, source and extracted packaged scripts | both passed (install, upgrade, remove, purge, abort, cleanup failure, unresolved role, manual recovery) |
| Isolated upgrade with fresh `state_check` | 0.0.1 → 0.1.0 kept identity, migrated v2 membership to v3 roles, restarted; failed cleanup aborted removal; remove retained state; purge deleted it |

Workspace 933 = the 928 of `a7abc60` plus the new seam tests (the in-file count rose from 12 to
19 client unit tests; other suites unchanged); `reconnect_deadline` rose from 10 to 12. Counts are
per run and not summed across overlapping runs. Change scope `0257bcb..a1702ad`: only
`crates/blueroute-client/src/lib.rs`, its `reconnect_deadline` test and documentation.

**Exact-revision CI** (read with `gh run view`; the backlog's "Ralph Bridge" instruction is
superseded by the maintainer's standing direction to use `git`/`gh` for CI evidence):
[run 43963097981](https://github.com/ekkus93/blueroute/actions/runs/43963097981) on `a1702ad`:
success (Rust, Desktop frontend); its "Run client reconnect deadline tests (RQR-01)" step reports
`12 passed`. Earlier: [43961287907](https://github.com/ekkus93/blueroute/actions/runs/43961287907)
on `08e50fa`, success, 12 passed. `8ca0ad3`, `7ad46e1`, `3fd226a` and `4df1339` were pushed
together with a later head and have no run of their own. The historical runs 43942151162 and
earlier passed while this defect existed and are not acceptance for it.

**Mocked layers and limits.** Broker daemons are scripted fakes on private session buses; package
tests mock the helper and systemd. Timings come from one development machine and CI's runner. A
call admitted in time may still be answered after the deadline (its answer loses); the microsecond
window between admission and the send inside one timely poll is inherent (DISPATCH-3). The zbus
partial-write cancellation limitation documented under RQR-01 is unchanged.

## RCD-CLOSEOUT — Requirement map and closure (2026-11-16)

| Requirement | Tasks | Source | Baseline / control failure | Final evidence |
| --- | --- | --- | --- | --- |
| DISPATCH-1 one budget | RCD-GUARD-01/04/05, RCD-TEST-04 | `Budget::starting_now` | (preserved) | first-poll, zero, short, overflow tests pass |
| DISPATCH-2 resumed polls | RCD-GUARD-02, RCD-TEST-01 | `poll_fn` expiry check in `negotiate_within` | broker regression (1,0,0)→(1,1,0) at `0257bcb` 3/3; Y1, Y2 | broker and seam resumed-expiry tests pass |
| DISPATCH-3 stage transitions | RCD-GUARD-03, RCD-TEST-03 | `Budget::admit` in `in_stages` | Y1, Y3 | `rcd_expiry_inside_a_poll…`, `rcd_a_retry_delay…` |
| DISPATCH-4 outcome precedence | RCD-GUARD-04, RCD-TEST-03/04/07 | post-outcome check | Y4 (4 seam tests incl. RQR-01's) | late compatible/incompatible/error tests pass |
| DISPATCH-5 typed errors | RCD-GUARD-05, RCD-TEST-04 | `last_error` by case | — | text and Unavailable checks in every timeout test |
| PRESERVE-1 owner/command safety | RCD-GUARD-06, RCD-TEST-06 | `validated_owner` unchanged for requests | — | restart, incompatible, lost-reply, `dbus_client`, `owner_race`; 0 Requests |
| PRESERVE-2 state and lifetime | RCD-GUARD-06, RCD-TEST-05 | `server_version` only on `Ok`; nothing spawned | — | cancellation test; version unchanged in every timeout and discarded setup |
| PRESERVE-3 boundaries | RCD-GUARD-06, RCD-TEST-06, RCD-VALIDATE | no change outside the client | — | all QR/watcher/daemon/desktop/TUI, package and frontend checks |
| TEST-1 public regression | RCD-BASE-03/04, RCD-TEST-01 | `rcd_an_expired_resumption…` with `Dispatches` | fails at `0257bcb` | passes 25/25 incl. under load |
| TEST-2 boundary matrix | RCD-BASE-04, RCD-TEST-02..06 | RCD-TEST table | — | all rows pass; raced setups reported and retried, never skipped |
| TEST-3 sensitivity | RCD-BASE-03, RCD-TEST-07 | `tmp/rcd-neg-controls.sh` (edits recorded) | baseline 3/3; Y1–Y4 | each guard caught by a test isolating it |
| EVIDENCE-1 exact candidate | RCD-VALIDATE-01..05 | `ci.yml` (suite already scheduled) | — | RCD-VALIDATE above; CI 43963097981 |
| EVIDENCE-2 reconcile claims | RCD-CLOSEOUT-01/03/04 | RC backlog and evidence corrections, SPEC.md, P5-005 docs, rustdoc | — | dated corrections in `a1702ad` and `08e50fa` |
| EVIDENCE-3 reviewed closure | RCD-CLOSEOUT-02/05 | independent review | — | review findings resolved in `4df1339` |

**Closure.** RRC-01 is **COMPLETE** as a software correction (software-only acceptance,
specification §5). With it, RQR-01's DEADLINE-3 and the RC items corrected above are met. The
existing QR, LC, FU and CR installed-system, physical and product/release gates are unchanged and
remain open in their own backlogs; this correction needs no equipment and makes no
hardware-support claim.

## RCD-REVIEW-01 — Client test-count correction and proof (2026-11-16)

Authority: [CODE_REVIEW_RECONNECT_COUNTS_TODO.md](CODE_REVIEW_RECONNECT_COUNTS_TODO.md)
(specification [CODE_REVIEW_RECONNECT_COUNTS_SPEC.md](CODE_REVIEW_RECONNECT_COUNTS_SPEC.md)).

**Finding.** The RCD-VALIDATE note above originally said the client unit-test count "rose from
12 to 19". The baseline actually had 14. The workspace totals (928 → 933), the 12-case
`reconnect_deadline` broker suite and every runtime result were right; only that comparison was
wrong. It is corrected in place above; this section holds the proof.

**Revisions and environment (RCN-BASE-01/02).** Work started at `e9ac106` (the reviewed
`3a9a631` plus the two counts plan documents; no code difference), clean tracked tree apart from
`.serena/` and `AGENTS.md`. Toolchain rustc 1.95.0 (59807616e 2026-04-14), cargo 1.95.0
(f2d3ce0bd 2026-03-21). Listings were taken in a detached worktree under the ignored `tmp/`,
checked out at each revision in turn (clean, 0 changed files each time), never in the main
checkout:

| Revision | Full SHA | Equivalence |
| --- | --- | --- |
| `a7abc60` (RQR-01 candidate) | `a7abc6004054e4c2bcc922fa3b22c103d532a810` | baseline |
| `0257bcb` (RQR-01 closeout) | `0257bcbf11f37276771a477e612529fc3e10f155` | `git diff --quiet a7abc60 0257bcb -- Cargo.toml Cargo.lock crates/blueroute-client` succeeds: identical client source and lock |
| `a1702ad` (RRC-01 candidate) | `a1702ad63ee1325fdfb3a945b0d63075efa8a06a` | candidate |
| `3a9a631` (RRC-01 closeout, reviewed) | `3a9a63114d369dd1eec1702f143272ac8d704aa9` | `git diff --quiet a1702ad 3a9a631 -- …` succeeds: identical |

**Commands** (in each checkout, exit status 0 every time):

```bash
cargo test -p blueroute-client --lib --locked -- --list
cargo test -p blueroute-client --lib --locked deadline_tests -- --list
```

**Harness totals** (the listings' own summary lines; the listings of each equivalent pair are
byte-identical):

| Scope | `a7abc60` = `0257bcb` | `a1702ad` = `3a9a631` | Change |
| --- | ---: | ---: | ---: |
| Whole `blueroute-client` library harness (`src/lib.rs` and `src/watch.rs`) | 14 tests | 19 tests | +5 |
| Selected by the `deadline_tests` filter (a subset of the line above) | 8 tests | 13 tests | +5 |
| Other library tests | 6 | 6 | 0 |

**Names (RCN-BASE-03).**

- Unchanged other six: `error_class_tests::error_names_are_exact_and_never_classify_untrusted_message_text`,
  `error_class_tests::lc_owner_only_a_missing_owner_retires_on_a_failed_final_check`,
  `error_class_tests::qr_queue_a_period_end_before_the_decision_withholds_and_one_after_retires_next`,
  `error_class_tests::qr_queue_an_overflow_is_typed_operational_and_names_its_limit`,
  `error_class_tests::version_mismatch_is_unavailable_and_unrelated_errors_remain_operational`,
  `watch::tests::reconnect_backoff_is_exponential_and_bounded`.
- Baseline eight `deadline_tests` (all still present): `rc_a_delayed_owner_lookup_is_bounded_by_the_whole_budget`,
  `rc_a_failure_after_the_deadline_keeps_its_diagnostic`, `rc_a_zero_budget_makes_no_attempt`,
  `rc_an_outcome_decided_after_the_deadline_loses_to_it`, `rc_an_unrepresentable_budget_does_not_overflow`,
  `rc_retry_delays_are_capped_and_stop_at_the_deadline`, `rc_the_budget_starts_at_the_first_poll`,
  `rc_timely_outcomes_decide_at_once`.
- Added five (the RRC-01 dispatch tests), none removed: `rcd_an_expired_resumption_does_not_poll_the_attempt_onward`,
  `rcd_a_retry_delay_ending_at_expiry_starts_no_lookup`, `rcd_a_stage_started_in_time_that_decides_late_loses`,
  `rcd_a_timely_resumption_negotiates_once`, `rcd_expiry_inside_a_poll_does_not_start_the_next_stage`.

**What this proves and what it does not.** A listing proves which tests the harness discovers,
not that they pass. Passing results stay attributed to the runs that recorded them: client
library 19 passed at `3fd226a` and `4df1339`, workspace 933 passed at `a1702ad`, 928 at
`a7abc60`. The library count is already inside the workspace count, and `deadline_tests` is
inside the library count; none of these are summed. The 12 `reconnect_deadline` cases are a
separate private-broker integration suite and were counted correctly.

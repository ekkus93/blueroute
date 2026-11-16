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

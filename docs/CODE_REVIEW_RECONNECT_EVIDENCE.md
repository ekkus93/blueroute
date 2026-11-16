# Code Review Reconnect Deadline Evidence

Versioned evidence for [CODE_REVIEW_RECONNECT_TODO.md](CODE_REVIEW_RECONNECT_TODO.md)
(specification: [CODE_REVIEW_RECONNECT_SPEC.md](CODE_REVIEW_RECONNECT_SPEC.md)). Software results
only, from private session brokers; nothing here is installed-system or physical qualification,
and no hardware-support claim follows from these timings.

## RC-BASE — Finding preserved and corrected expectations established (2026-11-16)

**Environment (RC-BASE-01).** Source `bd58ce2355816d7c7b956b9ed7dceedb4925044a`, the reviewed
baseline itself: no code difference. The tree was clean apart from the never-committed `.serena/`
and `AGENTS.md` and the two new reconnect documents. rustc/cargo 1.95.0, dbus-daemon 1.16.2,
zbus 5.19.0 (async-io executor), Debian 13 kernel 6.12.115. Private brokers via
`dbus-run-session`; test temporary state under the ignored workspace `tmp/` (`.cargo/config.toml`
sets `TMPDIR`). Guidance read: `CLAUDE.md`, `docs/SPEC.md` (client paragraphs), `docs/TODO.md`
P5-005, the queue/recovery specification, backlog and evidence, and this specification and
backlog.

### RQR-01 — `reconnect(timeout)` does not bound owner/version negotiation (RC-BASE-02)

- **Affected code.** `BlueRouteClient::reconnect` in `crates/blueroute-client/src/lib.rs` at
  `bd58ce2`:

  ```rust
  let deadline = Instant::now() + timeout;
  loop {
      match validated_owner(&self.connection).await {        // unbounded await
          Ok(owner) => { self.server_version = owner.version; return Ok(()); } // no expiry check
          Err(error @ ClientError::IncompatibleVersion { .. }) => return Err(error),
          Err(ClientError::Bus(_)) if Instant::now() < deadline => {
              Timer::after(RECONNECT_POLL_INTERVAL).await;   // full 100 ms, never capped
          }
          Err(ClientError::Bus(error)) => return Err(ClientError::ReconnectTimeout { .. }),
          Err(error) => return Err(error),
      }
  }
  ```

  The deadline is consulted only after a bus error. `GetNameOwner`, the owner proxy and `Version`
  are awaited without it, a reply that arrives after the deadline is accepted (compatible) or
  reported as incompatibility, and a budget below 100 ms still sleeps the full interval.
- **Review reproduction.** A client built with an immediate first Version reply, then
  `reconnect(50 ms)` with its Version reply held: still pending at 250 ms; releasing the reply
  produced `Ok(())` at about 251 ms, on three repeats.
- **History.** The method and its retry-only deadline check predate the queue/recovery (QR)
  changes; the QR work did not touch it. P5-005's "bounded reconnect" acceptance rested on the
  multi-second happy-path restart in `tests/dbus_client.rs`, which never holds a reply.
- **Scope.** The desktop/TUI shared watcher (`watch.rs`) does not call this method: it has its
  own connect/backoff path with its own bounds (CR-F2, QR-INTEGRATION). Only explicit callers
  of `BlueRouteClient::reconnect` are affected.

### Corrected expectations (RC-BASE-03/04)

`crates/blueroute-client/tests/reconnect_deadline.rs` (10 ignored private-broker tests). A raw
message-loop daemon on its own connection scripts each Version reply (immediate, held until
released, or a chosen version), counts ordinary `Request` calls, and can swallow a Request to
model a lost reply. A bus monitor counts the client's `GetNameOwner` calls. A 2 s watchdog bounds
every wait on the client and only turns a stall into a failure; it is never the client's
timeout. Scripted replies are always released before a scenario ends, scenarios serialize on one
mutex, and the daemon releases the name afterwards, so a failure cannot hang teardown. Nothing
touches host services, packages or Bluetooth.

Budget/tolerance choices: 50 ms budget with 150 ms scheduler tolerance (bound 200 ms, below the
250 ms the review saw pending); 30 ms budget with the service absent must finish within 90 ms,
strictly below one full 100 ms retry interval; zero budget within 150 ms.

Baseline run at `bd58ce2` plus the new test file, three repeats
(`dbus-run-session -- cargo test -p blueroute-client --test reconnect_deadline --locked --
--ignored`), each `5 passed; 5 failed`, identical every time:

| Test | Baseline result |
| --- | --- |
| `rc_a_held_version_reply_times_out_within_the_budget` | FAILED: `reconnect(50ms) was still pending after 2s` (the RQR-01 reproduction) |
| `rc_a_zero_budget_times_out_without_negotiating` | FAILED: `expected ReconnectTimeout(0ns): Ok(())` |
| `rc_an_absent_service_with_a_short_budget_caps_the_wait` | FAILED: took 100.89 / 100.95 / 101.09 ms, more than 90 ms |
| `rc_expiry_wins_over_a_late_compatible_reply` | FAILED: `a reply ready after the deadline (1.8) beat the timeout: Some(Ok(()))` |
| `rc_expiry_wins_over_a_late_incompatible_reply` | FAILED: `... (2.0) beat the timeout: Some(Err(IncompatibleVersion { .. 2.0 }))` |
| `rc_a_timely_compatible_reply_succeeds_and_records_its_version` | ok (control) |
| `rc_a_timely_incompatible_reply_fails_at_once` | ok (control) |
| `rc_a_restarted_daemon_is_reached_within_the_budget` | ok (control) |
| `rc_a_command_whose_reply_was_lost_is_never_replayed` | ok (control) |
| `rc_a_cancelled_reconnect_leaves_the_connection_and_subscriptions_working` | ok (control) |

The five failures are the corrected expectations; the five controls preserve the behavior that
was already right (timely success, immediate incompatibility, restart recovery, no replay,
cancellation safety).

## RC-DEADLINE — One whole-operation budget (2026-11-16)

**Source.** `3e05d98` (`fix(RC-DEADLINE)`), `crates/blueroute-client/src/lib.rs`:
`BlueRouteClient::reconnect` now delegates to the private `negotiate_within(timeout, attempt)`,
where one attempt is `validated_owner` (owner lookup, owner proxy and Version negotiation).

| Requirement | Implementation |
| --- | --- |
| DEADLINE-1 one budget | `Instant::now().checked_add(timeout)` once, in the async body, so on the first poll; the same absolute deadline for every attempt and delay. A budget too large to represent never expires (the old `Instant + timeout` panicked on it; negative control X1). |
| DEADLINE-2 bound every await | Each whole attempt (lookup, proxy setup, Version) races `Timer::at(deadline)` with `futures_lite::future::or`. On expiry the attempt future is dropped: its pending read-only call is abandoned, the connection is not closed. |
| DEADLINE-3 bounded delays | A bus failure sleeps until `min(now + 100 ms, deadline)`; the loop checks expiry before every attempt, so nothing is sent after it. Retry classification unchanged: only `ClientError::Bus` retries. |
| DEADLINE-4 outcome decision | After each attempt, expiry is checked **before** its outcome is looked at, so a reply that became ready with the deadline (compatible or incompatible) loses whatever order the branches were polled in. A timely incompatible reply returns at once without retries; every other typed failure also decides at once (unchanged). |
| DEADLINE-5 typed timeout | The existing `ReconnectTimeout { timeout, last_error }` (Unavailable) with the requested duration. `last_error` is the last bus failure's text when the expiry came during a retry delay; "the owner lookup or Version negotiation did not answer before the deadline" when an attempt was pending; "the daemon answered only after the deadline" when its outcome came too late; each with the previous bus failure appended when there was one. No bus error is fabricated; classification is by type, not text. |
| DEADLINE-6 zero/short | A zero budget is expired on the first check: no attempt, no state change. A short budget is capped like any other. |
| COMPAT-1 | Attempts are unchanged `validated_owner` calls: owner-bound, major/minor rules, no `Request`. Commands are never replayed. |
| COMPAT-2 | `server_version` is assigned only after `negotiate_within` returns `Ok`, which it does only before the deadline. Expiry or drop leaves it unchanged; no task is spawned, nothing outlives the future. |
| COMPAT-3 | No change to subscriptions, the shared watcher, the daemon, frontends, API 1.7, schema v3, join or sharing. |

The contract is documented on `reconnect`'s rustdoc and `negotiate_within`, in `docs/SPEC.md`
(client paragraph) and as a dated correction in `docs/P5-005-CLIENT.md`.

## RC-TEST — Timing, outcome and cancellation matrix (2026-11-16)

Private-broker suite `crates/blueroute-client/tests/reconnect_deadline.rs` (10 ignored tests) and
internal-seam unit tests `deadline_tests` in `crates/blueroute-client/src/lib.rs` (6 ordinary
tests, which drive `negotiate_within` with scripted attempts; the seam is private and test-only
use adds no public API).

| Scenario (spec table) | Tests | Result at `3e05d98` |
| --- | --- | --- |
| Held Version, short budget (RC-TEST-01) | `rc_a_held_version_reply_times_out_within_the_budget` | `ReconnectTimeout(50 ms)` in 50.058 / 50.065 / 50.069 ms (bound 200 ms) while the reply was still held; late reply released afterwards changed nothing; a fresh reconnect then recorded 1.8 |
| Delayed owner lookup (RC-TEST-02) | `rc_a_delayed_owner_lookup_is_bounded_by_the_whole_budget` (seam: lookup stage held 10 s, Version stage never reached) | timeout within 50 ms + 150 ms; 1 lookup, 0 Version |
| Zero budget (RC-TEST-03) | `rc_a_zero_budget_times_out_without_negotiating`; `rc_a_zero_budget_makes_no_attempt` | timeout in ~2 µs; 0 `GetNameOwner` seen by a bus monitor, no extra Version call, version unchanged; seam: 0 attempts |
| Service absent, budget < retry interval (RC-TEST-03) | `rc_an_absent_service_with_a_short_budget_caps_the_wait`; `rc_retry_delays_are_capped_and_stop_at_the_deadline` (30 ms and 250 ms) | 30.056 / 30.054 / 30.075 ms (bound 90 ms); exactly 1 lookup; `NameHasNoOwner` kept in `last_error`; seam: no attempt after expiry, finish within budget + 60 ms |
| Reply and timer both ready after expiry (RC-TEST-04) | `rc_expiry_wins_over_a_late_compatible_reply`, `..._incompatible_reply`; seam `rc_an_outcome_decided_after_the_deadline_loses_to_it` | the client is left unpolled past the deadline, the reply (1.8 or 2.0) is released and seen dispatched on the client connection, then one poll returns `ReconnectTimeout`; version stays 1.7 |
| Timely replies and restart (RC-TEST-05) | `rc_a_timely_compatible_reply_succeeds_and_records_its_version`, `rc_a_timely_incompatible_reply_fails_at_once`, `rc_a_restarted_daemon_is_reached_within_the_budget`, `rc_a_command_whose_reply_was_lost_is_never_replayed`; seam `rc_timely_outcomes_decide_at_once`, `rc_an_unrepresentable_budget_does_not_overflow` | success records 1.8; incompatibility in < 1 ms with 1 Version call and no retry; restart reached; 0 Requests during every reconnect; the swallowed command reached A once and B never |
| Cancellation, late reply, fresh operation (RC-TEST-06) | `rc_a_cancelled_reconnect_leaves_the_connection_and_subscriptions_working` | version unchanged after drop and late reply; no further lookup or Version for 300 ms; an active `events()` subscription on the same connection still receives the owner's event; a fresh reconnect succeeds |

Three repeats of the broker suite at `3e05d98`: `10 passed` each. Existing client suites at
`3e05d98`, all passed: `dbus_client` 1, `owner_race` 6, `event_owner_race` 6, `ownership_period`
12, `queue_saturation` 18, `owner_watch` 3, `watcher` 8; client unit tests 12.

### Negative controls

`tmp/rc-neg-controls.sh 3e05d98` (git-ignored helper; the edits are recorded here) creates a
detached worktree with its own `CARGO_TARGET_DIR`, applies one old behavior to
`negotiate_within`, runs `deadline_tests` and the broker suite, and reverts:

| Control | Restored behavior | Caught by (FAILED) |
| --- | --- | --- |
| X1 | the `bd58ce2` retry-only loop (`Instant + timeout`, unbounded attempt, full 100 ms sleep, no expiry check on success) | seam: zero budget, unrepresentable budget (panic), late outcome, capped delays, delayed lookup; broker: held Version, zero budget, absent service, both late-reply tests (5 controls still pass) |
| X2 | expiry checked only when the attempt did not finish | seam: late outcome; broker: both late-reply tests |
| X3 | retry delay not cut to the time left | seam: capped delays; broker: absent service |
| X4 | attempt not raced against the deadline | seam: delayed lookup; broker: held Version |

Each restored behavior is detected by both layers, and the late-reply tests fail only under the
controls that change the outcome decision, so they are not confirming the old bug.

## RC-CLOSEOUT-01 — Independent review (2026-11-16)

An independent reviewer read the specification and the code (not this record) at `bd6608f`,
ran both suites (6/6 seam, 10/10 broker) and checked zbus 5.19 internals. It found no correctness
defect against DEADLINE-1..6 or COMPAT-1..3, and confirmed: the deadline is taken on first poll;
`Proxy::new_owned` (lazy property caching) awaits no setup and spawns nothing; a dropped
`PendingMethodCall` removes its serial, so a late reply reaches only broadcast streams; the
post-attempt expiry check makes `future::or`'s attempt-first polling harmless; nothing is spawned
and the connection is never closed. Its six findings, all resolved in `af58349`:

| # | Finding | Resolution |
| --- | --- | --- |
| 1 | DEADLINE-5 text: an attempt *failing* after the deadline was described as "the daemon answered", dropping its bus error | The late outcome is now worded by kind; a late bus failure keeps its diagnostic. Test `rc_a_failure_after_the_deadline_keeps_its_diagnostic` |
| 2 | A call cancelled mid-write could leave a partial frame on the shared connection (zbus property, every cancelled zbus call) | Documented on `reconnect` and below as a limitation |
| 3 | The broker late-reply test relies on zbus completing a reply before broadcasting it | Assumption pinned in a comment; the seam test covers the precedence independently; `queue_saturation` pins zbus 5.19.0 |
| 4 | 60 ms slack on the short-budget timings could flake under load | Budget 20 ms with a bound of 95 ms (75 ms slack), still below one 100 ms retry interval so X3 stays detected; 150 ms slack for the 250 ms seam case |
| 5 | A panicking broker scenario skipped `release_name`, so later scenarios could cascade | `scenario` catches the unwind, releases the name, then resumes the panic |
| 6 | Nothing tested that the budget starts on first poll | Seam `rc_the_budget_starts_at_the_first_poll`; broker: a `reconnect(50 ms)` future left unpolled for 100 ms still succeeds |

**Controls at `af58349`** (`tmp/rc-neg-controls.sh`, now aborting loudly if an edit does not
apply, after a stale X2 anchor silently ran unmodified code once):

| Control | Seam FAILED | Broker FAILED |
| --- | --- | --- |
| X1 `bd58ce2` loop | 6 of 8 (all but timely outcomes and first poll) | 5 (held Version, zero, absent, both late replies) |
| X2 no expiry check before the outcome | late outcome, late failure diagnostic | both late replies |
| X3 uncapped retry delay | capped delays | absent service |
| X4 attempt not raced | delayed lookup | held Version |
| X5 deadline taken at creation | first poll | none: `reconnect` is itself `async`, so its inner future is only created on first poll; the broker check guards a regression in `reconnect` itself |

Results at `af58349`: seam 8/8, broker 10/10 on three repeats (held Version 50.11–50.23 ms,
absent service with 20 ms budget 20.15–20.19 ms, zero budget ≤ 11 µs).

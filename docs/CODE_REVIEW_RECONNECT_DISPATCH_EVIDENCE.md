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

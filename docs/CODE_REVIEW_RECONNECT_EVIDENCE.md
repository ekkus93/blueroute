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

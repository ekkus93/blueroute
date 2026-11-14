# Code Review Queue and Recovery Remediation Evidence

Versioned evidence for [CODE_REVIEW_QUEUE_RECOVERY_TODO.md](CODE_REVIEW_QUEUE_RECOVERY_TODO.md)
(specification: [CODE_REVIEW_QUEUE_RECOVERY_SPEC.md](CODE_REVIEW_QUEUE_RECOVERY_SPEC.md)). Software
results only; nothing here is installed-system or physical qualification.

## QR-BASE — Findings preserved and corrected expectations established (2026-11-14)

**Environment (QR-BASE-01).** Source `e7720a9` (the two queue/recovery documents only, on top
of the reviewed baseline `f957d80207930ccb481bec8f23cefd8bb2506cde`; no code difference). Clean
tree apart from the never-committed `.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, Node 26.13.0,
npm 11.20.0, Python 3.13.5, Debian 13 kernel 6.12.115, dbus-daemon 1.16.2, zbus 5.19.0 with
async-broadcast 0.7.2. Private brokers via `dbus-run-session`; test temporary state under the
ignored workspace `tmp/` (`.cargo/config.toml` sets `TMPDIR`). Guidance read: `CLAUDE.md`,
`docs/SPEC.md`, the lifecycle specification/backlog/evidence, this specification and backlog,
`docs/ADMINISTRATOR.md`, `docs/CR-F5-REMOVAL-CLEANUP.md`, `docs/CR-QUALIFY-PROCEDURE.md`.

### Pinned transport facts the analysis relies on (QR-BASE-02)

Read from the pinned zbus 5.19.0 sources:

- `connection/socket_reader.rs`, `receive_msg`: the single reader reads one message, dispatches a
  method reply or error to its waiting caller, then `broadcast_direct(..).await`s it into every
  matching stream before it reads the next message. A full matching stream therefore stops all
  reading on that connection, including replies to unrelated calls.
- `connection/mod.rs`, `add_match`: every match stream gets a broadcast channel of capacity
  `DEFAULT_MAX_QUEUED` (64) unless asked otherwise, created with `set_await_active(false)`.
  Subscriptions with the **same match rule share one channel** (`activate_cloned`), so an active
  receiver that is not read (for example in a retained handle) holds back every subscriber of that
  rule.
- Consequence: by the time a reply is in hand, every earlier message is queued in its streams
  (the LC-OWNER ordering argument still holds), but a reply can only be read while every matching
  stream has room for the messages ahead of it.

### RLC-01 — ownership-queue saturation blocks final verification

- **Trigger.** A subscription bound to owner A sees A emit an item. A answers the item's Version
  check and then releases and reacquires the name N times (2N ownership notices). The consumer is
  paused from the Version call until all of that has completed, then resumes.
- **Affected path.** `crates/blueroute-client/src/lib.rs`, `EventSubscription::next_change`. The
  Version future is ready on resumption, so `future::or` returns it without polling the ownership
  branch; the final `current_owner` (`GetNameOwner`) then waits for a reply queued behind
  notices that nothing reads while the ownership stream is full. Nothing else drains it, so the
  call never returns.
- **Earlier tasks.** LC-OWNER-02/05/06 and LC-INTEGRATION-01 (EPOCH-2/4): their tests used at most
  four notices, below the queue capacity.
- **Signal selection (QUEUE-1).** The same circular wait exists for the owner's own signals: the
  Event and StateChanged streams are not read while a verification reply is awaited, so more
  signals than the reader can hold ahead of the reply stall the subscription.
- **Corrected expectations.** `crates/blueroute-client/tests/queue_saturation.rs` (ignored,
  private bus; not yet scheduled in CI). Ordering is established on the daemon side: each
  release and reacquire is the fake daemon's own broker round trip, so the broker has queued all
  notices for the client before the burst is marked complete; nothing depends on the client's
  possibly blocked reader. Bursts are finite; the fake daemon's own unfiltered queue is enlarged
  so the fixture never stalls itself.
- **Baseline result** (`dbus-run-session -- cargo test -p blueroute-client --test
  queue_saturation --locked -- --ignored` at `e7720a9`, 3 runs, identical each time): 3 passed,
  6 failed.

  | Test | Burst | Result at baseline |
  | --- | --- | --- |
  | `qr_queue_a_burst_below_capacity_retires_event` / `_state` | 20 cycles, 40 notices | pass (control) |
  | `qr_queue_a_burst_at_capacity_retires_event` | 32 cycles, 64 notices | pass (boundary control) |
  | `qr_queue_a_burst_just_above_capacity_retires_state` | 33 cycles, 66 notices | *"no OwnerLost within 2s of resuming: None"* |
  | `qr_queue_forty_cycles_after_the_reply_retire_event` / `_state` | 40 cycles, 80 notices | *"no OwnerLost within 2s of resuming: None"* |
  | `qr_queue_forty_cycles_then_drop_keep_the_connection_working` | 40 cycles | same; fails before the drop |
  | `qr_queue_signals_queued_ahead_of_a_reply_are_all_delivered` | 96 signals | *"stalled after 0 items"* |
  | `qr_queue_a_signal_flood_ends_the_subscription_without_stalling` | 320 signals | *"stalled after 0 items"* |

  The threshold sits exactly at the queue capacity: 64 notices pass and 66 fail. That is the
  failure the review describes, explained by the reader, not by timing.
- **Scope.** One private broker, fake daemon; Event and StateChanged subscriptions. No physical
  or installed-system claim.

### RLC-02 — the documented manual recovery leaves an unresolved client record

- **Trigger.** Member/None for network N plus a client record naming N (the RFU-02 state). The
  administrator confirms there is no PAN link and deletes N's owned NetworkManager state, then
  follows step 4 of `docs/ADMINISTRATOR.md` "Removal with an unresolved role": for a `member`
  network with no role it says to delete the `network` and `peer` lines only.
- **Affected path.** `docs/ADMINISTRATOR.md` step 4. The retained record names a network with no
  membership, which `removal_scope` correctly classifies as OrphanClientRecord, so removal still
  exits 1. The cleanup classifier is right (it refuses uncertainty); the instructions are
  incomplete. `docs/CR-QUALIFY-PROCEDURE.md` A3 "Unknown active role, recovery" already deletes
  both, so only the guide disagrees.
- **Earlier tasks.** LC-CLEANUP-07 (recovery by restoring a verified role, a different path) and
  LC-CLOSEOUT (documentation review).
- **Corrected expectation.** `qr_recovery_the_documented_recovery_of_an_unknown_role_completes`
  in `apps/daemon/src/removal.rs`: production stores, a fake backend standing in for the
  independently completed NetworkManager cleanup, and a helper that edits `memberships-v1`
  exactly as the guide's step 4 instructs. A foreign network's state must stay reported.
- **Baseline result** (`cargo test -p blueroute-daemon --lib --locked qr_recovery`): fails with
  *"the documented recovery left removal failing: RemovalReport { …, unresolved: 2,
  unresolved_links: 0, unresolved_roles: 1, … }"*. Like LC-BASE-04, this unit test runs in the
  ordinary suite, so it is committed with its correction (QR-RECOVERY) rather than left failing
  in CI.

### Isolation (QR-BASE-04)

- Broker tests use only the `dbus-run-session` private session bus; the client and fake daemon
  are session connections. No system bus, BlueZ, NetworkManager or package database is reached.
- Bursts and floods are finite (at most 40 cycles, 320 signals); every wait on the client is
  bounded by 2 seconds; daemon-side waits poll flags the test sets.
- Removal tests use the fake `ClientLeaveBackend` and production stores in a `tempfile` directory
  under the ignored workspace `tmp/`, removed on drop.

# Code Review Lifecycle Remediation Evidence

Versioned evidence for [CODE_REVIEW_LIFECYCLE_TODO.md](CODE_REVIEW_LIFECYCLE_TODO.md) and
[CODE_REVIEW_LIFECYCLE_SPEC.md](CODE_REVIEW_LIFECYCLE_SPEC.md). Each entry names the exact source
revision it was taken at. Software results here are not installed-system or physical evidence.

## LC-BASE — Findings preserved and corrected expectations established (2026-11-14)

**Environment.** Source `e167198` (lifecycle docs only on top of the reviewed baseline
`75c1a01746557bb13f525a55642b497e3bd5f81d`; no code difference). Clean tree apart from the
never-committed `.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, zbus 5.19.0 (async-io), Debian
13 kernel 6.12, private brokers via `dbus-run-session`. Guidance read: `CLAUDE.md`,
`docs/SPEC.md`, the follow-up and lifecycle specifications and backlogs.

### RFU-01 — a returning owner bypasses retirement (LC-BASE-02/03)

- **Trigger.** A subscription bound to owner A sees A emit an item. Inside that item's Version
  check A releases the name, B takes and releases it, and A takes it back, then A replies.
- **Affected path.** `crates/blueroute-client/src/lib.rs`, `EventSubscription::next_change`: the
  Version reply races the ownership watch with `future::or`, which polls the reply first, and the
  final check compares only `GetNameOwner`'s unique name, which is A again.
- **Earlier tasks.** FU-02-02/03 and FU-INTEGRATION-02 accepted A→B handover only; OWNER-1..4.
- **Corrected expectation.** `crates/blueroute-client/tests/ownership_period.rs`. The fake daemon
  is a raw message loop, so the test knows when its reply was sent; after replying it calls a
  barrier object on the client's own connection, which travels behind the reply, so once it is
  answered the reply and all four ownership notices are queued at the client. The consumer is
  polled only until A receives the Version call and then paused until that acknowledgement.
  No sleep establishes the order.
- **Baseline result** (`dbus-run-session -- cargo test -p blueroute-client --test
  ownership_period --locked -- --ignored`, at `e167198`): 1 passed, 2 failed.
  - `lc_owner_a_returning_owner_does_not_deliver_its_obsolete_event`: *"the old period's Event
    was not retired: Some(Ok(Event(HealthChanged(Degraded))))"*.
  - `lc_owner_a_returning_owner_does_not_deliver_its_obsolete_state_change`: *"the old period's
    State was not retired: Some(Ok(State { revision: 7 }))"*.
  - `lc_owner_a_signal_queued_before_a_release_and_reacquire_is_not_delivered` passed: there the
    Version call is issued only after the notices are queued, and the ownership branch of the
    race wins because its item is already ready. It is kept as a maintained regression for
    EPOCH-2, but it is not evidence of the defect.
- **Scope of the reproduction.** One private broker, fake daemon; Event and StateChanged
  deliveries share `next_change`. No physical or installed-system claim.

### RFU-02 — an unknown active role bypasses client cleanup (LC-BASE-02/04)

- **Trigger.** The production membership store holds `Member` with role `none` for a network,
  and the production client-record store holds a record for the same network. Cleanup runs
  without Bluetooth.
- **Affected path.** `apps/daemon/src/removal.rs`, `removal_scope`: `client` is
  `role == Some(Client)`, so the unknown role becomes `NetworkScope { client: false, .. }` and the
  network is cleaned as host-only.
- **Earlier tasks.** FU-04-01/02/03/06/07/09, FU-INTEGRATION-03; REMOVE-1..5.
- **Corrected expectation.** `lc_cleanup_an_unknown_active_role_with_a_client_record_stays_unresolved`
  in `apps/daemon/src/removal.rs` seeds both stores through their production `save`, runs the
  production `run_removal_cleanup` against the in-memory NetworkManager/BlueZ fake, and requires
  failure, the owned profile and address still present, no disconnect, and the membership and
  record unchanged.
- **Baseline result** (`cargo test -p blueroute-daemon --lib --locked lc_cleanup`, at
  `e167198`): failed with *"an unknown active role was cleaned as host-only: RemovalReport {
  networks: 1, failed_networks: 0, remaining: 0, unresolved: 0, unresolved_links: 0, bluetooth:
  false }"*. The test is unit-level and runs in every workspace test run, so it is committed with
  its fix (LC-CLEANUP) rather than ahead of it; this baseline failure is its record.

### Isolation (LC-BASE-05)

- The ownership tests use only the `dbus-run-session` session bus, which they serialize with a
  file-local lock; every connection is dropped at the end of its scenario.
- The cleanup tests use a per-test temporary state directory and the in-memory backend; no
  NetworkManager, BlueZ or system bus is reached.
- Package-level tests (`packaging/debian/test-maintainer-lifecycle.sh`) already use a temporary
  `DPKG_ROOT` with a mocked helper, so they cannot reach host buses. LC-INTEGRATION extends them.

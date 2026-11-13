# P6-009 — Joined-client restart reconciliation (design, PROPOSED)

**Status: accepted by the maintainer on 2026-11-09 (see "Maintainer answers"); implementation proceeds stage by stage.** It closes the
remaining software gap before the P6-004 join gate may open (see the P6-004 addendum, "Review
finding R2" and "joined-client leave"). Production `JoinNetwork` stays fail-closed throughout.
Verify every code reference against the current tree before implementing.

## Findings from reading the code

**F1 (blocking): startup reconciliation assumes every persisted `Member` is a host.**
`SingleStarNetworkOperations::reconcile_on_startup` loads the registry and, for the current
network, calls `start_host` (NAP, advertisement, bridge, local star address). Its own comment says
a persisted `Member` "currently represents a host created by this daemon" and that a persisted role
must be added before client join is enabled. Today this is safe only because join is closed. A
joined client restarted with the gate open would try to **host** a network it joined, creating a
bridge, address and advertisement for someone else's network.

**F2: a joined client's runtime state lives only in memory.** After a committed join the live
`JoinPanLink` and `JoinIpLease` are retained in `LinuxJoinRuntime`, and the control session in
`ControlSupervisors`. A restart loses all three. A `Member` with no retained state is
indistinguishable from a host, so `LeaveNetwork` falls back to host leave.

**F3: durable ownership evidence already exists for most resources.**
- IP: the client address is deterministic from the `NetworkId` (`Ipv4StarAddressPlan::first_client`)
  and the NetworkManager profile/address carries BlueRoute ownership metadata (`user.data`, scoped
  by `NetworkId`). `NetworkManagerBackend::addresses()` lists owned addresses; the backend can
  remove one by owner.
- PANU: BlueZ exposes the live attachment for a device (`current_panu_attachment`,
  `require_matching_panu`, `disconnect_matching_panu`). The `bnepN` name changes across reconnects,
  so it must be re-observed, never persisted as identity.
- Trust: after commit the host is trusted and its key pinned in the **member** stores, so the
  supervised session can re-authenticate after a restart through the member path with no pending
  record.
- Missing: the durable **role** and which BlueZ device is the host.

**F3b: the membership file is versioned.** `membership_store.rs` is at schema v2 with a migration
chain, so adding a field is a normal v3 migration.

## Proposed design

### D1. Persist the role
Membership schema v3 adds `role: Host | Client` to each `NetworkMembership`, written when `Member`
is committed (create ⇒ `Host`, join ⇒ `Client`). Migration v2→v3 maps every existing `Member` to
`Host`, which is correct because join has never been enabled. A `Member` with a missing or unknown
role is **not acted on**: no mutation, health `Degraded`, a diagnostic naming the cause.
`reconcile_on_startup` switches on role; the host path is unchanged; the client path is D3.

### D2. Persist only what cannot be re-derived
A small durable client record per network: the host `NodeId` (already in the member stores) and an
optional BlueZ device path as a **hint**. Interface names, leases and sessions are never persisted;
the lease is re-derived from the `NetworkId`, the interface and attachment are re-observed. The hint
may be stale (adapter renumbering); the locator (D2 of the P6-004 addendum) is the fallback and the
pinned TLS handshake is what authenticates whatever is reached.

### D3. Startup procedure for `role = Client` (observe, then decide)
Never blindly mutate. Runs as a **background task** so a slow radio cannot block daemon start or
D-Bus; health is `Reconnecting` while it works. Single host rule: exactly one approved pinned host
for the network from the member stores, else fail closed (`Degraded`, no action).

| Observation | Action |
| --- | --- |
| PANU up for the host, owned lease present on that interface | Rebuild the lease/link handles, start the supervisor (dial with the member pin). Healthy on `established`. |
| PANU up, owned lease missing | Re-apply the deterministic lease if the segment is free (conflict-checked); on conflict stay `Degraded`. Then supervisor. |
| PANU down | Re-establish via hint then locator (bounded backoff, shared with P6-010), then lease, then supervisor. |
| Owned NM profile present but PANU gone | Keep it and report `Reconnecting`; it is reused when PANU returns or removed by leave. Never removed by a timer. |
| Anything not carrying this network's ownership or not matching the recorded host | Ignore. Foreign state is never touched. |

### D4. Leave must not depend on memory
`leave_joined` becomes observation-based for a `Client` role: stop the supervisor if any, remove
**this network's** owned NM address/profile by ownership metadata, disconnect PANU only when the
live attachment matches the recorded host, then clear `Member`. Retained handles become an
optimization, not a requirement, so a client restarted into `Reconnecting` can still leave cleanly.

### D5. Policy
- Never auto-leave: durable `Member` survives every failure; retries are bounded exponential
  backoff (P6-010 owns link-loss while running; P6-009 owns restart; they share one reconnector).
- Health and events surface the state; the desktop UI already renders `Reconnecting`.
- No role inference from Bluetooth addresses, names, IPs or advertisements.

### D6. Interaction with the join gate
`hardware_verified` must stay unsettable in production until D1 to D4 are implemented and tested;
the gate-opening change must assert it.

## Staged plan (each stage its own commit with CI)

1. **S1** v3 schema with `role`, migration, `create`/`join` write it, startup reconcile dispatches
   on role and refuses to host a `Client` (fixes F1).
2. **S2** durable client record (host hint) written at commit, cleared on leave.
3. **S3** observation-based client leave (D4) with a fake backend.
4. **S4** client reconciler (D3 table) over a `ClientRuntime` trait with fakes; table-driven tests
   for every row.
5. **S5** background reconnect task with backoff and health/event reporting.
6. **S6** hardware checklist in `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`: reboot with a live PANU,
   adapter unplug/replug, `bluetoothd` and NetworkManager restart, host restart.

## Open questions for the maintainer

1. **Legacy migration:** is mapping every existing `Member` to `Host` acceptable?
2. **Host hint:** persist the BlueZ device path as a hint, or always rediscover through the locator?
3. **Retries:** retry forever with a capped backoff, or stop after N attempts and wait for the user?
4. **Stale owned artifacts:** after long failure keep them and stay `Degraded` (proposed), or
   remove them after a timeout?
5. **Gate:** confirm S1 to S4 are mandatory before the join flag may be enabled.

## Out of scope

Multi-star topology, roaming between hosts, and automatic host re-selection (one approved host per
network in v1). Physical acceptance is recorded separately.

## Maintainer answers (2026-11-09)

1. Mapping every existing `Member` to `Host` in the v2→v3 migration: **accepted**.
2. The BlueZ device path is persisted **as a hint** only; the locator remains the fallback.
3. **Changed from the proposal:** the reconnector does **not** retry forever. It makes a bounded
   number of attempts (N, with capped exponential backoff), then stops, leaves `Member` intact,
   reports `Degraded` with an actionable message, and **waits for the user** (an explicit retry or
   leave). A retry action must be available through the daemon API and the UI.
4. Stale owned NetworkManager artifacts are kept and reported `Degraded`; no timer removes them.
5. S1 to S4 are **mandatory** before the join flag may be enabled.

### S1 schema (implemented 2026-11-09)
Membership schema v3 adds a per-network `role` field: `host`, `client` or `none`. Migration maps a
v2 `member` row to `host` and a `not-member` row to `none`. In memory the role is
`Option<MembershipRole>`; it is set when `Member` is committed (create ⇒ `Host`, join ⇒ `Client`)
and cleared when the node leaves. A `Member` with no role is never acted on: startup reconciliation
and host leave refuse it, and the daemon reports `Degraded`.

**S1 done (software):** `MembershipRole` in `blueroute-core`; `membership_store` schema v3 with the v2→v3 migration (members→`host`, others→`none`; a role on a non-member or an invalid role is rejected); create writes `Host`, join commit writes `Client`, both leaves clear it. `reconcile_on_startup` and host `leave_network` now require a persisted `Host` role: a `Client` or unknown role is refused (`InvalidState`, nothing hosted or torn down, durable membership untouched, daemon reports `Degraded`). Tests cover migration, round trips, invalid roles, both refusals and role set/clear on create and leave. **S2 to S6 remain open and the join flag stays unsettable.**

**S2 done (software):** `apps/daemon/src/client_record.rs` adds `ClientRecord` (network, host `NodeId`, optional BlueZ device path) and `ClientRecordStore` (owner-only file `client-record-v1` under `client-record/`, single record, 1024-byte limit, strict parsing, a record for another network is ignored). The join coordinator writes it after membership and the pin are durable and `finalize_commit` succeeded, and clears it after a completed joined-client leave. The hint is advisory: a failed write or clear does not change the join or leave outcome, because the locator remains the fallback and the pinned handshake authenticates the host. A failed or refused join writes nothing, and a failed leave keeps the record. **S3 to S6 remain open and the join flag stays unsettable.**

**S3 done (software):** joined-client leave no longer depends on memory. The coordinator decides from the **persisted** role (`Member` with `Client`), not from retained runtime state, so a client restarted into any state can leave. `apps/daemon/src/client_leave.rs` adds `leave_observed` over a fakeable `ClientLeaveBackend` (production: `SystemClientLeave` over NetworkManager and BlueZ). It stops the supervisor, then re-derives work from ownership metadata: addresses and NetworkManager profiles carrying this `NetworkId` are removed (the backend's owner filter is not trusted, it is re-applied); the PANU link is disconnected only when the live attachment is for the recorded host device (retained link or S2 hint) **and** is proven ours, either equal to the exact retained attachment or running on an interface that carries this network's ownership. Anything else, including a link to another device, a non-PANU role, or a replacement link on an unowned interface, is left alone. The link is disconnected **first**, while the owned profile still proves it is ours, so every step is idempotent and a failed leave resumes correctly on retry. A PANU link that cannot be proven ours is deliberately not disconnected (foreign-state rule); leave still completes. Tests are table-driven over each case plus a coordinator test of a restarted client leaving with nothing retained. `JoinRuntime::leave_joined` now takes the hint, and `joined_client` was removed from the trait. **S4 to S6 remain open and the join flag stays unsettable.**

**S4 done (software, logic only):** `apps/daemon/src/client_reconcile.rs` adds the D3 observe-then-act pass `reconcile_client_once` over a fakeable `ClientRuntime` trait (`observe`, `lease_segment_free`, `reestablish_panu`, `apply_lease`, `start_supervisor`), plus `single_approved_host` (exactly one node that membership trusts **and** the pin store has a key for, else fail closed). One pass returns `Started`, `Reconnecting` (a retryable transport failure, per `ErrorKind::is_retryable_transport_failure`) or `Degraded` (needs a person: address conflict, ambiguous host, failed authentication, lease prefix mismatch). It never removes anything: an owned profile with PANU gone is kept and reported `Reconnecting`, and a lease on another interface or owned by another network is ignored. Only a PANU-role attachment with a peer counts as the client link. Table-driven tests cover every D3 row and the classification. **Not done in S4:** the production `ClientRuntime` (NetworkManager, BlueZ, locator, supervisor) and the startup wiring and retry loop; both belong to S5 together with the bounded backoff, the retry action and health/events. S5, S6 remain open and the join flag stays unsettable.

**S5a done (software, not yet wired):** `apps/daemon/src/client_runtime.rs` adds the production `SystemClientRuntime` (obtained from `LinuxJoinRuntime::client_runtime()`, sharing its supervisors and retained links). It observes through BlueZ (`observe_panu` for the recorded device) and NetworkManager ownership metadata, checks the deterministic segment with the same collision rule as join, re-establishes PANU with the recorded device first and the untrusted locator as fallback, applies the deterministic owned lease, and starts the supervised pinned control session through the helper join now also uses (`start_supervised_control`), so a reconnect dials exactly as a join does. Every mutating call is behind the **same closed gate** as join (`require_hardware_verified`, one definition); a test proves all three are refused with the gate closed and nothing is started. `apps/daemon/src/client_reconnect.rs` adds `run_reconnect` with `ReconnectPolicy` (default 8 attempts, 2 s doubling to a 60 s cap, saturating): it stops after the attempt budget with `GaveUp` and waits for the user, ends at once on `Degraded`, honours a stop request before an attempt and during a wait (sleeps are sliced to 250 ms), and reports progress through a callback for health and events. **Still open for S5b:** spawning the run at startup for a persisted `Client`, the health/event reporting, the daemon API and UI retry action, and cancelling the run on leave. S6 remains open and the join flag stays unsettable.

**S5b done (daemon and API, software):** API 1.3 adds the read-only `GetClientLink` (`ClientLink(Option<ClientLinkState>)`) and the PolicyKit-gated `RetryClientReconnect`. `apps/daemon/src/client_link.rs` adds `ClientReconnector`: at startup a persisted `Client` is reconnected on a **background thread** (so a slow radio never blocks daemon start or D-Bus) using the single-approved-host rule, the S2 hint and the deterministic lease; the run is bounded by `ReconnectPolicy` and ends in `Established`, `NeedsAttention` (a plain-words reason, never a raw diagnostic) or `GaveUp`. Durable membership is never touched. `RetryClientReconnect` starts a fresh run and is refused when the node is not a joined client or a run is active. Published health is the **worse of** the capability monitor's reading and the reconnection state (`Reconnecting` while trying, `Degraded` for `NeedsAttention`/`GaveUp`), so the 15 s monitor cannot hide it and it clears on leave. Startup no longer reports a persisted client as a failed host restore. Leave cancels the run first; the reconnector's runtime wrapper refuses to start a session after cancel and takes down one that finished starting during it, so a late run cannot bring a session back after the leave tears it down. Because the runtime shares the join gate, a persisted client today ends in `NeedsAttention` rather than touching Bluetooth. Tests cover the state machine, health merge, cancel and the guard; the D-Bus integration test checks the read-only query. **Open:** CLI and desktop UI for the retry action (S5c), S6 hardware checklist, and a leave-while-connecting hardware check. The join flag stays unsettable.

**S5c done (CLI and desktop, software):** the CLI adds the read-only `blueroute network link`, which shows the joined client's link in plain words (the daemon's reason text is escaped like any other diagnostic field; `--json` prints the typed state or `null`), and `blueroute network reconnect` for `RetryClientReconnect`. The desktop bridge adds a read-only `client_link` snapshot and a third individually granted Tauri mutation, `retry_client_reconnect` (no arguments; the daemon enforces PolicyKit). The dashboard shows the link only for a joined client and offers "Try connecting again" only when the link is `gave_up` or `needs_attention`; a refused retry shows the actionable reason. Tests cover CLI parsing and rendering, the bridge wire shapes the UI depends on, the fixed Tauri command mapping and the four UI cases (no link, connecting/connected without a button, gave-up retry and refresh, needs-attention with a refused retry). None of this is demonstrated against a live daemon with a real joined client, which cannot exist while the join gate is closed. **S6 remains open and the join flag stays unsettable.**

**S6 done (checklist written, not run):** [HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md#p6-009-joined-client-restart-checklist-s6-not-started) lists the two-machine scenarios: daemon restart with a live PANU, client reboot, host absent then retry, host restart, adapter replug, `bluetoothd` and NetworkManager restart (each followed by a daemon restart), leave while connecting, and leave after a restart with nothing retained, each with expected results and foreign-state checks. It needs a gate-open build, so it cannot run until P7 and P6-004 hardware acceptance. It also records that link loss while the daemon keeps running is P6-010/P13 work. **All software stages S1 to S6 are written; P6-009 stays SOFTWARE COMPLETE / HARDWARE DEFERRED and the join flag stays unsettable.**

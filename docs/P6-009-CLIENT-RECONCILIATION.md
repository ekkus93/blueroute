# P6-009 — Joined-client restart reconciliation (design, PROPOSED)

**Status: proposal for maintainer review, 2026-11-09. Nothing here is implemented.** It closes the
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

# P6-004 — Join-network orchestration and prerequisite boundary

## Scope

P6-004 is the product-level `JoinNetwork` operation for the first single-star BlueRoute LAN. A successful join must eventually perform, in order:

1. select the intended discovered BlueRoute network and establish its PANU data-plane link;
2. obtain/apply conflict-safe IP configuration for that PANU interface;
3. establish an authenticated BlueRoute control session that binds the remote peer to stable BlueRoute identity and network membership policy;
4. commit local durable membership as `Member` only after all required runtime steps succeed.

This document records an ordering conflict found while implementing P6-004 and the fail-closed boundary used until its prerequisites are complete.

## Dependency conflict discovered during implementation

The repository roadmap originally placed two required P6-004 mechanisms after P6-004 itself:

- **P6-005** defines the one-star IPv4 allocation policy, conflict detection, and address assignment. P6-005 now supplies that prerequisite through deterministic `NetworkId`-derived addressing and NetworkManager conflict observation/application.
- **P7-001/P7-004** still need to select and implement the authenticated inter-node control transport/session lifecycle.

The main specification is explicit that source IP, Bluetooth display metadata, and similar transport observations are not proof of BlueRoute identity. P6-003 additionally established that Bluetooth pairing/trust and BlueRoute `NodeId` approval are independent facts.

Therefore P6-004 cannot safely be completed by substituting any of the following for the missing authenticated control session:

- Bluetooth MAC address;
- BlueZ `Device1` object path;
- Bluetooth name or alias;
- P6-002 manufacturer-data advertisement;
- source IP address;
- `Trusted=true` in BlueZ;
- an unauthenticated peer-asserted `NodeId`.

Likewise, P6-004 must not invent a silent fixed PANU address that bypasses P6-005's conflict policy and then later treat that prototype behavior as a protocol assumption.

## Transactional join coordinator

P6-004 introduces a daemon-side `JoinNetworkOperations` boundary and a transactional coordinator. Its required step order is:

```text
preflight
  -> establish PANU
  -> configure IP
  -> start authenticated control session
  -> commit durable local Member state
```

The daemon updates `DaemonStatus.current_network` only after the coordinator returns success.

The coordinator deliberately does not persist `Member` before the runtime is usable. The core domain transition remains:

```text
NotMember -> Joining -> Member
```

but the durable commit occurs only after PANU, address configuration, and control-session establishment have all succeeded.

## Rollback policy

Every completed runtime step has an explicit reverse operation. Failure is rolled back in reverse order:

```text
control session -> IP configuration -> PANU attachment
```

Examples:

- IP failure disconnects the PANU and leaves no durable membership.
- Control authentication/session failure removes IP state, disconnects PANU, and leaves no durable membership.
- A membership-file read/state/persistence failure after runtime establishment stops the control session, removes IP state, disconnects PANU, and does not claim membership.
- Cleanup failures are surfaced in diagnostics together with the primary error; they are never silently discarded.

The existing daemon-wide mutation guard continues to serialize privileged local API mutations so create/trust/join operations cannot race independent membership-file read/modify/write cycles.

Until P6-009 is implemented, durable `Member` state is also not treated as proof that runtime PAN/IP/control state exists after a restart. A `JoinNetwork` request for a network already marked durable `Member` fails explicitly instead of returning a false idempotent success.

## Production fail-closed behavior while blocked

`LinuxJoinRuntime::preflight` currently returns `CapabilityUnavailable` before Bluetooth or durable-state mutation because **P7 authenticated control-session support** is still unavailable.

P6-005 now implements the client-side IP stage behind that gate: once PANU exists, `LinuxJoinRuntime::configure_ip` derives the first-client address from `NetworkId`, checks active local prefixes, and applies/removes the BlueRoute-owned NetworkManager address/profile. Returning a typed preflight error remains intentional until the remote BlueRoute identity can be authenticated.

The remaining production runtime methods also fail explicitly rather than using placeholder `Ok(())` cleanup implementations. This prevents a future preflight change from accidentally turning an unwired rollback path into a silent success.

The PANU transport primitive itself is already implemented and physically proven by P4-005. P6-004 does not duplicate that adapter. With P6-005 now supplying conflict-safe address planning/application, the remaining production activation dependency is the authenticated P7 control-session contract.

## Deterministic coverage

Tests cover:

- successful fake-runtime orchestration commits `Member` only after preflight, PANU, address, and control steps;
- address failure disconnects PANU and persists no membership;
- control failure removes IP state, disconnects PANU, and persists no membership;
- durable `Member` state without observed runtime does not fake an idempotent join success; it fails with an explicit P6-009 reconciliation dependency;
- production preflight fails closed before unsafe partial join;
- `JoinNetwork` remains behind the existing `org.blueroute.modify` PolicyKit boundary;
- authorization denial prevents join dispatch;
- daemon status changes only after a successful join-operation return.

## Completion status

P6-004 is **blocked, not complete**. The orchestration/state/rollback boundary and P6-005 address prerequisite are implemented, but production acceptance cannot be claimed until authenticated P7 control bootstrap exists and a real compatible node joins without manual Bluetooth/network shell commands.

The remaining implementation dependency is the minimum P7 control-plane work required for authenticated join; P6-004 should then be resumed for production wiring and physical acceptance.

## Addendum (2026-11-07, PROPOSED): production wiring design

**Status: proposal. The four open questions below were answered by the maintainer on 2026-11-07 (see "Maintainer answers"), but a code review of the trust layer found a conflict in D1/D3 (see "Review finding R1") that must be resolved before implementation. Nothing here is implemented.** It records the decisions needed before the three production stubs (`preflight`, `establish_panu`, `start_control_session`/`stop_control_session`) can be wired. Production `JoinNetwork` stays fail-closed throughout. Where this text names an existing primitive, its contract is the one documented in P6-003, P7-001, P7-003 and P7-004; verify against the code before implementing.

### D1. Who the host is: operator-confirmed identity, not the advertisement

`JoinNetwork { network }` keeps its wire shape. The runtime resolves the intended host **only** from durable local state that a human established:

1. The host's `NodeId`, its control-TLS SPKI SHA-256 and its `NetworkId` are obtained from the host operator through a separate channel (`blueroute peer local-key` on the host; see P7-003-INDEPENDENT-KEY-COMPARISON.md).
2. The joiner records durable P6-003 approval for that `NodeId` on that `NetworkId` and an independently confirmed fingerprint.
3. A `JoinNetwork` for a network with **no** approved, confirmed host record fails before any Bluetooth mutation with a typed, actionable error (for example `PermissionDenied`/`InvalidState` with a diagnostic naming the missing step). Exactly one approved host per network is allowed in v1 (single star); zero or more than one fails closed rather than choosing.

The P6-002 advertisement, BlueZ name/alias, MAC, `Trusted=true` and source IP are never used to choose *whom to trust*. They may only help find the BlueZ device to **connect to** (D2), and a wrong device simply fails authentication in D3.

### D2. Which BlueZ device: untrusted locator, authenticated afterwards

A `PeerLocator` boundary maps a `NetworkId` to candidate BlueZ `PeerHandle`s (devices currently advertising that network). Its result is an untrusted hint. Rules:

- A locator that finds zero or several candidates fails closed with a "nearby network not found / ambiguous" error; it never picks one silently.
- Establishing the PANU link to a candidate is the **only** mutation allowed before authentication. It is reversed by `disconnect_panu` if D3 fails.
- No TCP connection is made, and no address is configured as a BlueRoute member, until D3's pre-dial checks pass. (IP configuration for the PANU interface is required to reach the NAP and happens before the TLS dial; it is owned, rolled back on failure and never treated as membership.)

### D3. Authentication: the existing pinned dial, nothing weaker

`start_control_session` uses `connect_registered_panu_control_session`, which already revalidates durable approval and the exact SPKI pin before the SYN, binds the socket to the owned PANU source address, then performs mutual TLS 1.3 and the exporter-bound Hello. The join runtime must:

- supply the confirmed pin from D1 (`PinnedControlTlsPeer`) and never a pin learned from the connection being authenticated;
- treat first-contact pin enrollment (`enroll_confirmed_control_candidate`) as a separate, explicitly operator-confirmed step with its own deadline, not an implicit side effect of join;
- map every failure (TLS, pin mismatch, Hello, approval revoked) to rollback in the documented reverse order and surface cleanup failures alongside the primary error.

### D4. Session ownership and link loss

`JoinControlSession` today names only a `NetworkId` and owns nothing, so a successful join would leave an authenticated session with no owner. Required before membership may be committed:

- The daemon owns **one supervisor task per joined network**. It holds the registered TLS session, runs `run_registered_panu_supervision_until_stopped` (heartbeats, approval revalidation, PANU-loss observation) and reports loss through the existing health/event path.
- `JoinControlSession` becomes a handle to that task. `stop_control_session` cancels it and **waits for it to finish** (bounded), because dropping a future is not cleanup (see the join transition guard).
- `Member` is committed only after the supervisor reports the session established. Loss after commit marks the runtime unhealthy and does not silently rejoin; reconnect and restart reconciliation stay with P6-009/P6-010.
- Leave (P6-008) stops the supervisor before tearing down IP and PANU.

### D5. Staged enablement

1. Land D2's locator, `establish_panu`, and D4's supervisor behind fakes, with deterministic tests for every rollback path, a missing/ambiguous host, a wrong peer, revoked approval mid-join and supervisor stop.
2. Keep `preflight` returning `CapabilityUnavailable` unless an explicit, default-off build or configuration flag marks the runtime hardware-verified. CI never sets it.
3. Hardware acceptance (two machines, no manual Bluetooth/network shell commands, key confirmed out of band, rollback on induced failure) is recorded in `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`. Only that evidence may remove the flag.

### Open questions for the maintainer

- **Single approved host per network:** confirm v1 should reject zero or several rather than offer a choice. (A future multi-star design would revisit this.)
- **Operator inputs:** is entering the host `NodeId` and fingerprint in the CLI/GUI acceptable for the first release, or should an invitation code/QR format be specified first (SPEC §18 mentions it as future work)?
- **Failure after commit:** confirm that supervisor loss marks the runtime unhealthy but keeps durable `Member` (reconciled by P6-009/P6-010) instead of auto-leaving.
- **IP before authentication:** confirm that configuring the owned PANU address before the TLS dial is acceptable, given it is rolled back and never counted as membership.

### Consequences for P12

The desktop Join workflow (P12-006) stays disabled until D5 step 3. The UI must collect the host identity (D1) and present the fingerprint comparison before enabling Join; it must not present advertised networks as joinable by themselves.

### Maintainer answers (2026-11-07)

1. Exactly one approved host per network; reject zero or several. **Accepted.**
2. Typed host `NodeId` and fingerprint (CLI first; the GUI shows the fingerprint in groups and asks the user to confirm). Invitation codes/QR are a later, separately specified feature. **Accepted.**
3. Supervisor loss after commit keeps durable `Member`, marks the runtime unhealthy and defers reconnect/reconciliation to P6-009/P6-010. **Accepted.**
4. Configuring the owned PANU address before the authenticated dial is acceptable (derived from `NetworkId`, owned, conflict-checked, rolled back, never membership). **Accepted.**

### Review finding R1 (2026-11-07): the existing approval layer cannot authorize a joiner

Reading `apps/daemon/src/peer_trust.rs` and `outbound_control.rs` shows that D1 step 2 and D3 assume something the code does not provide:

- `require_peer_approved`, `trust_peer`, `forget_peer`, `approved_control_pins` and `with_approved_pinned_tls_peer` all require that the **local** durable `current_network` equals the target network, and that the target network already has a `NetworkMembership` record. A prospective joiner has no current network, so these return `InvalidState`/no-current-network.
- `connect_registered_panu_control_session` calls `require_approved_pinned_tls_peer` before the TCP dial and again after the handshake. For a joiner that call can never succeed, so the pinned dial cannot be used for an outbound join as it stands.
- The join coordinator commits `Member` only at the very end (a deliberate rule, above), so the joiner is never "current" while it authenticates.

The approval/pin layer was built for a node that is already in the network (the host admitting peers, or an existing member reconnecting). The joiner side has no equivalent.

Options (needs a maintainer decision):

- **A. Joiner-side pending-join record (recommended).** Add a durable, separate record: "intended join target = (NetworkId, host NodeId, confirmed SPKI)", created by an explicit PolicyKit-authorized operator action and cleared on success, failure or leave. It is **not** membership and does not change `current_network`. Add joiner-specific methods (for example `require_pending_join_host_pin`) used by the outbound dial and supervision in place of the member-only check, so the existing member-only functions keep their stricter contract untouched. This keeps the invariant that only a completed join creates `Member`.
- **B. Persist a `Joining` membership early and let it count as current.** Smaller code change but it changes what `current_network` means, leaks half-joined state into status/UI and the host-side trust checks, and risks treating an unauthenticated, in-progress join as an approved network. Not recommended.
- **C. Authenticate first with a one-shot, in-memory pin supplied with the join request.** Avoids new durable state, but loses revocation-before-commit semantics and survives neither daemon restart nor audit. Not recommended.

Until one is chosen, D1 step 2 and D3 are **not implementable as written**, and no join wiring code should be started.

### Decision on R1 (2026-11-07): option A accepted

The maintainer chose **A, the joiner-side pending-join record**. D1 step 2 and D3 are amended as follows (this supersedes their original wording):

- **D1 (amended):** the operator records the confirmed host `(NetworkId, NodeId, SPKI SHA-256)` as the single pending-join host through an explicit, PolicyKit-authorized action. The member-only approval layer is not used before the join. A different host is never replaced silently (clear first); a node already in a network cannot record or use one.
- **D3 (amended):** the outbound dial and its supervision check `PendingJoinOperations::require_host_pin` (exact network, node and key; still in no network) instead of `require_approved_pinned_tls_peer`. The TLS handshake and exporter-bound Hello still prove possession of that key. The member-only functions are unchanged. After the join commits `Member`, the pin must be enrolled into the member pin store (host-side semantics) in the same durable step or the join does not commit, then the pending record is cleared.
- **Cleared** on success, on any rollback and on leave, so a stale record cannot authorize a later attempt.

Implemented so far (software only, gate still closed): `apps/daemon/src/pending_join.rs` (`PendingJoinHost`, `PendingJoinStore`, `PendingJoinOperations`) with an owner-only, versioned, size-bounded, single-record file that rejects symlinks, zero identities, corrupt or future formats. **Also done (API 1.2):** `RecordJoinHost`/`ClearJoinHost` commands behind `org.blueroute.modify`, daemon dispatch, and `blueroute peer set-join-host <network> <node> <fingerprint>` / `blueroute peer clear-join-host`. **Also done (joiner dial authorization):** instead of duplicating the TLS session types, `DurablePeerTrustOperations::with_pending_join` adds a fallback inside `with_approved_pinned_tls_peer` (and therefore `require_approved_pinned_tls_peer`, the pre-dial and post-handshake checks, per-frame revalidation and supervision). It applies only while the local node is in **no** network and a pending host is recorded, and then accepts exactly that host's network, node and key; every member-side decision (including inbound approval lists and enrollment) is unchanged. Tests cover an end-to-end mutual-TLS session from a non-member joiner to a member host, a mismatching recorded key (no TLS bytes are sent) and the fallback switching off once a network is current. **Consequence for commit:** once `Member` is committed the fallback stops applying, so the host's key must already be approved and pinned in the member stores in the same durable step, or supervision would drop the session at once. **Also done (enroll at commit):** `apps/daemon/src/join_authorization.rs` (`JoinCommitAuthorization`, `PendingHostCommitAuthorization`) is called by the coordinator after the `Member` transition is prepared and before membership is saved: it trusts the host in the in-memory registry, enrolls the confirmed key in the durable pin store (a conflicting existing pin blocks the commit), then clears the pending record. If the membership save then fails, only a pin this commit added is removed. After any failed attempt that touched the runtime the pending host is cleared, so retrying needs the host to be recorded again (a refusal before any runtime mutation keeps it). The coordinator treats a missing authorization as legacy/test-only; **production wiring supplies it in `main.rs`, and the join gate must not be opened by any path that omits it.** **Also done (locator and `establish_panu`, gate still closed):** `apps/daemon/src/peer_locator.rs` (`PeerLocator`, `BluezPeerLocator`, `select_single_peer`) plus `BluezBackend::discovered_network_peers` choose the BlueZ device advertising the network from powered adapters, as an untrusted hint: zero candidates is a retryable `PanFailure`, several is `InvalidState` and nothing is guessed. `LinuxJoinRuntime::establish_panu` locates, connects PANU, validates the returned attachment (a link for the wrong peer or role is torn down, not adopted) and returns the `JoinPanLink`. `LinuxJoinRuntime` now carries `hardware_verified` (always `false`; settable only in tests) and both `preflight` and `establish_panu` refuse while it is off, so no Bluetooth link is made; when open, `preflight` additionally requires the operator-confirmed host for that network. **Also done (supervised control session, gate still closed):** `control_supervisor.rs` (`ControlSupervisors`) owns one worker thread per network that creates, supervises and drops the TLS session entirely on that thread (it borrows trust state), reports establishment through a waker-based handshake, turns panics into errors so `start` cannot hang, refuses a second session per network, records `Running`/`Ended(reason)` for health reporting and joins the worker on `stop`. `join_control.rs` (`run_join_control`, `JoinControlConfig`) is the production worker body: pinned dial from the owned PANU address, mutual TLS 1.3 with exporter-bound Hello, then `run_registered_panu_supervision_until_stopped`. `LinuxJoinRuntime::start_control_session` takes the pin from the operator's record and starts it; `stop_control_session` stops and joins it; both refuse while `hardware_verified` is off. Lifecycle is tested with fakes; the real dial has no hardware-free test. **Commit ordering amended:** the pending record is now cleared by `finalize_commit` **after** membership is saved (clearing earlier would leave a window where per-second supervision, authorized by neither the joiner fallback nor the member stores, could drop the live session). If finalizing fails the coordinator restores the previous membership, undoes the pin and rolls the runtime back; tested. `stop` blocks its caller for at most one supervision tick.

### Review finding R2 (2026-11-07): joined-client leave does not exist

`SingleStarNetworkOperations::leave_network` tears down the **host** (NAP bridge, address, advertisement) only; the TODO already notes joined-client leave awaits P6-004. After a successful join nothing would stop the supervisor, remove the owned PANU address and profile or disconnect PANU, so the user could not cleanly leave and `leave` would act as if hosting. Before the gate may open, leave must, for a joined client: stop the supervisor (`ControlSupervisors::stop`), remove the owned IP lease, disconnect the matching PANU, then clear `Member` (reverse of join). It needs the live `JoinPanLink`/`JoinIpLease`, so the runtime must retain them per network (not yet done), and restart reconciliation (P6-009) must cover a `Member` with no supervisor. These are blocking gaps for enabling join.

**Not yet done:** a joiner variant of `connect_registered_panu_control_session` and the supervision loop, the peer locator, `establish_panu`, the supervisor and the enrolment-on-commit step.

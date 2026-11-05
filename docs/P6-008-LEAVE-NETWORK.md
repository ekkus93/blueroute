# P6-008 — Leave-network operation

## Scope

`LeaveNetwork` (already classified as an `org.blueroute.modify` PolicyKit mutation) now dispatches to `NetworkOperations::leave_network`, implemented by `SingleStarNetworkOperations`, which owns the host runtime that must be torn down.

## Behavior

1. Claim the daemon lifecycle guard, shared with `CreateNetwork`, so create and leave cannot interleave.
2. Load durable membership. If the daemon is not an active `Member`, return success with no side effects (idempotent).
3. Stop the runtime first: the discovery advertisement, the BlueZ NAP, the BlueRoute-owned address, and the owner-scoped NetworkManager bridge, using the existing `StarHostRuntime::stop_host`. It refuses to act on mismatched owned state.
4. Only after teardown succeeds, transition `Member -> Leaving -> NotMember`, clear per-peer `member` flags, and atomically persist.
5. The daemon's `current_network` status is cleared only after the operation returns success.

`Leaving` is a transient state and, like `Joining`, is never persisted.

## Failure and retry semantics

- Runtime teardown fails: durable state is unchanged (still `Member`), the typed error is returned, and a retry re-attempts teardown.
- Durable commit fails after teardown: the error says teardown happened and that `LeaveNetwork` should be retried. The retry repeats the idempotent stop and then commits.

## Trust-retention policy

Leaving retains:

- the known-network record (name and identity), so the network can be rediscovered or re-created cleanly;
- explicit peer trust approvals from P6-003. SPEC requires forgetting/revoking to be a separate, clear user action (`ForgetPeer`).

Leaving clears each peer's `member` flag for the departed network, since those peers are no longer members of an active network on this node.

## Deterministic coverage

- leave stops the runtime, commits `NotMember`, and keeps the known-network record;
- leave is idempotent and a no-op without membership;
- failed teardown keeps `Member` and a later retry succeeds;
- trust is retained and peer membership is cleared;
- create after leave succeeds without conflicting with the old record;
- `LeaveNetwork` is denied without PolicyKit authorization, dispatches once authorized, and clears `current_network` (real-broker test under `dbus-run-session`).

## Not yet proven (why P6-008 stays in progress)

- **Physical acceptance.** "Leaves no stale data-plane state" has not been checked on hardware. A hardware run must show the bridge, address, NAP, and advertisement gone after leave, and repeated leave being a no-op. CI cannot satisfy this.
- **Client-side runtime.** Production `JoinNetwork` is blocked on P7 (see `P6-004-JOIN-ORCHESTRATION.md`), so there is no production PANU/IP/control-session state to tear down on a joined client yet. When P6-004 is unblocked its runtime must be added to this teardown.
- **Restart.** The host runtime handles (NAP, advertisement) are held in memory. After a daemon restart with durable `Member` state, `stop_host` finds no active runtime and returns success without re-observing the system. Real cleanup after restart depends on P6-009 reconciliation.

# P6-001 — Create-network lifecycle

## Scope

P6-001 is the first operation in the single-star BlueRoute LAN product slice. `CreateNetwork` turns a logical BlueRoute network into one locally hosted PAN star on a NAP-capable Linux node.

This task owns:

- generation of a new logical `NetworkId` that is independent of the human-readable network name;
- capability-gated selection of the local node as a NAP host;
- creation of a BlueRoute-owned NetworkManager bridge;
- assignment of the host address for one deterministic initial IPv4 segment;
- BlueZ NAP registration against that bridge;
- durable local membership commit only after the required runtime networking succeeds;
- D-Bus `CreateNetwork` dispatch and `DaemonStatus.current_network` update.

It does **not** claim the later P6 responsibilities:

- P6-002 defines/discovers the over-the-air BlueRoute network identity;
- P6-003 defines peer join approval and trust;
- P6-004 implements the client-side join operation;
- P6-005 owns route-conflict detection and complete per-client IPv4 allocation policy;
- P6-008 owns the public leave-network lifecycle;
- P6-009 owns daemon-restart reconciliation of an already-hosted network.

## Logical identity

The network name is a `DisplayName` and is never an authorization identity. A new 128-bit `NetworkId` is generated from the Linux kernel random source. Generation is retried if the candidate collides with a remembered network.

The generated `NetworkId` is also used only as deterministic input for owned runtime names/addresses; changing a display name never changes network identity.

## Initial hosted-star network state

The default configuration already reserves `10.201.0.0/16` with `/24` PAN segments. P6-001 deterministically selects one `/24` from that pool from the new `NetworkId` and assigns host address `.1` to the local bridge.

The bridge name is:

```text
brb-<first 8 lowercase hex characters of NetworkId>
```

This stays below Linux's 15-character interface-name limit.

P6-001's deterministic selection is enough to establish a local subnet for the first hosted-star operation. It is not a substitute for P6-005: route-conflict detection, allocation retries/selection policy, and client address assignment are deliberately deferred to that task.

## Capability gate

`CreateNetwork` requires `NodeCapabilities::can_host_pan() == Some(true)`.

- Explicit `false` returns `CapabilityUnavailable` with a NAP-specific explanation.
- Unknown capability also fails closed rather than optimistically attempting to host.
- No computer model or adapter model is used to decide support.

The production daemon obtains this capability from the system capability probe already implemented in P4-010.

## Runtime ordering and durable commit

The production operation performs the following sequence:

1. reject a conflicting concurrent create transition;
2. verify NAP capability;
3. load and validate durable membership state;
4. reject creation when the daemon is already an active member of another network;
5. generate the logical network identity;
6. derive the owned bridge and initial host address;
7. connect to the BlueZ and NetworkManager system-bus backends;
8. select a powered Bluetooth adapter;
9. create/reconcile the BlueRoute-owned bridge;
10. assign/reconcile the BlueRoute-owned host address;
11. register BlueZ NAP on that bridge;
12. transition local membership `NotMember -> Joining -> Member` in memory;
13. atomically persist the stable `Member` fact;
14. update the daemon's local API `current_network` only after the operation returns successfully.

Transient `Joining` state is never persisted. This preserves the P3 persistence invariant that a restart reads stable facts rather than treating an interrupted operation as complete.

## Failure and rollback policy

The operation is fail-closed.

- A capability failure occurs before network mutation.
- Bridge/address/NAP failures do not commit membership.
- If a later setup step fails, already-created BlueRoute-owned runtime state is removed in reverse dependency order.
- If the durable membership commit fails after runtime setup, the host runtime is torn down.
- Cleanup errors are surfaced in diagnostic context; they are not swallowed.
- The runtime retains ownership information when cleanup fails so a later cleanup attempt is not converted into a silent no-op.
- A second `CreateNetwork` while already a member is rejected instead of creating another profile/NAP.

No shell command output is parsed by production code; the operation composes the direct BlueZ and NetworkManager adapters from P4.

## API behavior

P5-007 authorization still runs before command dispatch. An authorized `CreateNetwork` calls the injected `NetworkOperations` implementation. Success returns `Response::Ack`; subsequent `Status` reports the created `NetworkId` as `current_network`.

The real-broker authorization integration test verifies that an unauthorized create never reaches the network operation, while an authorized create reaches it, returns `Ack`, and updates `Status.current_network`.

## Acceptance

Deterministic CI proves ordering, capability rejection, no durable commit after runtime failure, duplicate-create rejection, deterministic bridge/subnet derivation, D-Bus dispatch, and PolicyKit ordering.

Physical acceptance is recorded in `docs/P6-001-HARDWARE-EVIDENCE-2026-09-01.md`. On `debiancb1`, the production `CreateNetwork` path persisted network `26ed3f29d622ae9c5c68635f4d548bbe`, created `brb-26ed3f29` with `10.201.41.1/24`, registered the NAP, and accepted the real `arisu` PANU so server interface `enxf4d10870b786` became a kernel member of that exact bridge.

P6-001 is complete. Discovery, approval, join orchestration, automatic client addressing, leave, reconciliation, and reconnect remain later P6 tasks.

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

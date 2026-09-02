# P6-005 — IPv4 allocation for one star

## Scope

P6-005 turns the deterministic address placeholder introduced by P6-001 into the initial production IPv4 policy for a single BlueRoute star. It owns:

- selection of the private IPv4 pool and per-star prefix size;
- deterministic derivation of the star subnet from stable `NetworkId`;
- active-local-network conflict detection before address mutation;
- host and first-client address assignment;
- conflict retry policy for `CreateNetwork`;
- cleanup behavior that leaves no allocation lease or stale NetworkManager profile.

P6-005 does **not** authenticate a peer or complete P6-004. The authenticated control-session dependency remains in P7.

## Initial private address policy

The version-1 default pool is:

```text
10.201.0.0/16
```

and each single-star PAN receives one `/24` segment. `Ipv4AddressPool::validate` now requires the entire configured pool to be contained in RFC1918 private space (`10/8`, `172.16/12`, or `192.168/16`). CGNAT, documentation, public, and otherwise non-RFC1918 pools fail closed.

For one `NetworkId`, the segment selector is the first 32 bits of the ID modulo the number of segments in the configured pool. The initial address roles are:

```text
segment network + 1  -> NAP/host
segment network + 2  -> first PANU/client
```

The `/30` lower bound already enforced by configuration leaves room for both addresses plus network/broadcast semantics.

## Why the subnet stays a pure function of NetworkId

A joining node learns the logical network identity from P6-002 before an authenticated P7 control session exists. If the host silently selected a different fallback subnet for the same `NetworkId`, the client would need another unauthenticated bootstrap protocol or durable allocation record merely to learn its initial IP configuration.

P6-005 avoids that problem. The mapping:

```text
(NetworkId, configured pool) -> segment, host address, first-client address
```

is pure and deterministic. No Bluetooth MAC, display name, interface name, source IP, or peer-supplied identity influences allocation.

## Conflict detection

`NetworkManagerBackend` now exposes a read-only `IpNetworkObservationBackend` boundary. It observes active IPv4 address and route prefixes from NetworkManager `IP4Config.AddressData` and `IP4Config.RouteData` on live devices.

This observation API is intentionally separate from `IpNetworkBackend::addresses()` and `routes()`. Those existing methods enumerate only BlueRoute-owned state so reconciliation and cleanup cannot accidentally adopt or remove foreign configuration. Conflict detection, by contrast, must see Wi-Fi, Ethernet, VPN, administrator, and other non-BlueRoute prefixes.

Before the production host creates a bridge or registers a NAP, it normalizes the candidate segment and rejects any overlap with an active non-default IPv4 prefix. The ordinary `0.0.0.0/0` default route is not a conflict because Linux can install a more-specific connected route alongside it.

Malformed NetworkManager address/route data fails closed instead of being ignored.

## Conflict retry policy

The segment remains tied to `NetworkId`, so a collision does not cause BlueRoute to mutate the subnet mapping. Instead `CreateNetwork` discards the conflicting candidate identity and generates another random `NetworkId`.

Up to 16 distinct candidate identities are attempted. Every `AddressConflict` is returned by `LinuxStarHostRuntime::start_host` before bridge/Bluetooth mutation for that candidate. If all attempts conflict, creation returns a typed `AddressConflict` with the last conflict in its diagnostic context.

Non-address errors are never converted into retries.

## Client address application

The P6-004 `LinuxJoinRuntime` now contains the real P6-005 client address step:

1. derive the same star plan from `NetworkId` and the configured pool;
2. observe active local IPv4 prefixes;
3. reject a local overlap before mutation;
4. apply `segment + 2` to the PANU interface through the NetworkManager backend;
5. on rollback, remove the address and the BlueRoute-owned generic NetworkManager profile.

Production `JoinNetwork` still fails in preflight before Bluetooth mutation because P7 authenticated control bootstrap remains unavailable. P6-005 therefore supplies the IP prerequisite without weakening the P6-003/P6-004 trust boundary.

## No transient allocation database

P6-005 adds no lease file, DHCP database, subnet registry, or other durable transient state. The network identity determines the intended subnet; live NetworkManager state determines whether it is currently safe to use.

P6-009 restart reconciliation must prefer already-observed BlueRoute-owned runtime state over blindly recomputing and replacing an active address.

## Deterministic and hardware acceptance

Unit coverage verifies:

- deterministic host and first-client addresses;
- RFC1918-only pool validation;
- normalization and overlap detection for broader/narrower prefixes;
- default-route non-conflict behavior;
- stateless repeated planning;
- permissive observation of irrelevant NetworkManager route metadata while required malformed fields fail closed;
- `CreateNetwork` retry to a fresh `NetworkId` after `AddressConflict` with no durable record for the rejected candidate.

The hardware probe is:

```bash
cargo run -p blueroute-linux --example ipv4_allocation_probe --locked -- <optional-network-id>
```

It uses the production NetworkManager backend to apply and remove the planned host address on a temporary BlueRoute-owned bridge twice. Each cycle verifies that the owned address/profile are present while active, absent after cleanup, and that the selected segment is conflict-free again after cleanup. No `ip`, `nmcli`, or manual network mutation is used as part of the operation.

Live Debian acceptance on `debiancb1` is recorded in `docs/P6-005-HARDWARE-EVIDENCE-2026-09-02.md`. The probe derived `10.201.101.0/24` for test network `65656565656565656565656565656565`, applied `10.201.101.1/24` twice through the production NetworkManager backend, cleaned the owned state after each cycle, and finished with `P6-005 IPv4 allocation probe PASS`.

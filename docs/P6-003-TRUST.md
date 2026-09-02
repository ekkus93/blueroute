# P6-003 — Join approval and trust workflow

## Scope

P6-003 establishes the initial explicit trust boundary used before a BlueRoute peer may join a network. It deliberately keeps Bluetooth transport trust separate from BlueRoute network authorization.

This task implements:

- composable BlueZ pair-then-trust behavior for a selected Bluetooth peer;
- `TrustPeer { node: NodeId }` as explicit BlueRoute approval for the daemon's current network;
- durable persistence of that approval;
- `ForgetPeer { node: NodeId }` as explicit revocation;
- a fail-closed `require_peer_approved(network, node)` guard for P6-004 join orchestration;
- PolicyKit enforcement before local trust/revocation mutations reach the daemon operation layer.

P6-003 does **not** establish PANU, allocate an address, start the inter-node control session, or mark the approved peer as an active network member. Those are P6-004 and later control-plane tasks.

## Two independent trust layers

BlueRoute distinguishes two facts:

1. **Bluetooth pairing/trust** — BlueZ `Device1.Pair` and `Device1.Trusted` establish transport-level Bluetooth trust for one selected BlueZ peer.
2. **BlueRoute network approval** — the durable BlueRoute membership registry records that a specific stable `NodeId` has been explicitly approved for the current `NetworkId`.

Neither fact substitutes for the other. In particular, BlueRoute never derives `NodeId` from a Bluetooth MAC address, `Device1` object path, name, alias, RSSI, pairing state, or P6-002 advertisement data.

The Linux helper `pair_and_trust_bluetooth_peer` composes the already-idempotent P4 pairing and trust operations for a caller that has independently selected the correct BlueZ peer. It creates no BlueRoute membership state.

## Explicit BlueRoute approval

The existing local API command:

```text
TrustPeer { node: NodeId }
```

is now implemented by the daemon. The operation:

1. crosses the existing `org.blueroute.modify` PolicyKit boundary;
2. rejects the local node's own `NodeId`;
3. requires the daemon to have exactly one current stable BlueRoute network;
4. inserts or updates the peer record for that network with `trusted=true`;
5. atomically persists the membership registry;
6. leaves `member=false`.

The last rule is important: approval is permission to attempt the later join workflow, not proof that the networking/control-plane join succeeded.

Repeated approval of an already-approved peer is idempotent and does not rewrite durable state unnecessarily.

## Revocation

`ForgetPeer { node: NodeId }` is also implemented behind the same PolicyKit mutation boundary. It removes the peer record from the current network, which revokes both BlueRoute trust and any remembered peer-membership fact. Repeated forget of an unknown peer is idempotent.

P6-003 does not silently modify BlueZ `Trusted` when BlueRoute approval is revoked. Bluetooth transport trust and BlueRoute authorization are intentionally separate state. A later user-facing policy may offer an explicit option to revoke both layers together.

## Join guard

`DurablePeerTrustOperations::require_peer_approved(network, node)` is the mandatory fail-closed guard intended for P6-004 before PANU/IP/control-session mutation.

It rejects when:

- the daemon is not currently a stable member of a network;
- the requested network is not the current network;
- the peer `NodeId` is absent or not explicitly trusted.

An absent/unapproved peer returns `AuthenticationFailed`; the D-Bus boundary maps authentication failures to `AccessDenied` rather than a generic internal failure.

P6-004 must call this guard before treating an authenticated remote `NodeId` as eligible to join. It must not replace the guard with checks of Bluetooth `Trusted`, a MAC address, a display name, or discovery metadata.

## Durable mutation serialization

Create-network, peer approval, and peer revocation all replace the same membership registry file. P6-003 therefore serializes privileged daemon mutations at the local D-Bus service boundary. Concurrent mutating requests fail clearly rather than performing overlapping read-modify-write cycles that could overwrite durable state.

Read-only requests remain available while a mutation is in progress. The guard is process-local; crash/restart reconciliation remains P6-009/P6-010.

## Security limitations

P6-003 provides the initial **explicit local approval policy**, not cryptographic binding between a BlueZ peer and a BlueRoute `NodeId`.

The authoritative identity binding belongs to the authenticated inter-node control plane described in P7. Until that exists:

- pairing alone never grants BlueRoute approval;
- BlueRoute approval alone never marks a peer joined;
- P6-002 advertisements remain unauthenticated hints;
- a source IP, Bluetooth address, name, alias, or pairing state is never accepted as proof of `NodeId`;
- the later join path must fail closed if the peer cannot present an authenticated approved `NodeId`.

## Deterministic coverage

Tests cover:

- the existing P4 pairing/trust primitives continue to cover idempotent BlueZ pairing and trust behavior;
- durable peer approval without setting `member=true`;
- rejection of unapproved peers;
- revocation returning the peer to the rejected state;
- PolicyKit denial preventing `TrustPeer` dispatch;
- authorized `TrustPeer` and `ForgetPeer` dispatch.

Physical P6-003 acceptance should reuse the already-proven P4-004 Bluetooth pairing backend and additionally demonstrate that a real daemon `TrustPeer` request persists the intended `NodeId` as trusted while leaving it non-member, and that `ForgetPeer` removes that approval.

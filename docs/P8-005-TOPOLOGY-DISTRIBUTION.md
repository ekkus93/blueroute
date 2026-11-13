# P8-005 — Authenticated topology distribution (design, PROPOSED)

**Status:** ACCEPTED as direction, implementation DEFERRED (maintainer, 2026-11-12). Docs only: no
code, wire format or invariant changes yet. Production `JoinNetwork` stays closed throughout.

## Why this is needed

Every remaining P8 item (P8-002 wiring, P8-003 runtime forwarding, P8-005 distribution and
application, P8-006/007/008 acceptance) needs a multi-segment topology graph in the running
daemon. Today nothing builds one.

## Findings from reading the code

- **F1. The wire deliberately carries no topology.** `ControlMessage` is `Hello`, `HelloAck`,
  `Heartbeat` and `Goodbye` only (`crates/blueroute-protocol/src/control.rs`). Route, topology
  and membership messages fail strict decoding (P14-003, checked invariant "deny-by-absence").
  `HelloFacts` already carries a `segment` prefix and `direct_neighbors`, but they are
  unauthenticated claims and nothing consumes them.
- **F2. The consumers exist and are tested.** `desired_node_routes`, `plan_route_reconciliation`,
  `execute_route_reconciliation`, `reconcile_observed_topology_routes` (P8-008 failover),
  `desired_ipv4_forwarding` and the Linux route and forwarding executors all take a
  `TopologyGraph`. With no distributed topology, each daemon would only ever see its own star.
- **F3. Hop-by-hop TLS does not authenticate a remote origin.** A pinned TLS session proves who
  the *neighbor* is. In A–B–C, C talks to B; if B relays "A has a link to X", C cannot tell
  whether A said it or B invented it.
- **F4. Pins only cover direct peers.** A joined client pins exactly one host (P6-004 D1;
  `single_approved_host`), and a host pins its approved clients. No node holds the keys of
  members it is not directly connected to.
- **F5. The role model is one role per network.** `MembershipRole` (P6-009 S1) is `Host` or
  `Client` per network. A routing node in an interconnected star hosts one segment and is a
  client of another within the same network, which the model cannot express.
- **F6. Everything here sits behind the join gate.** With join closed, no client and hence no
  multi-node network can exist outside tests. This work is software-only until the gate opens.

## Proposed design

### D1. Link-state, never routes
Nodes exchange **link-state** (which authenticated neighbors they have on which segment) and each
node computes its own routes locally with the existing planner. No message ever carries a route,
so "reject foreign/non-member routes" holds by construction, and a member cannot push a route
into another node's kernel. The P8-009 decision (BlueRoute-owned planning, no Babel) is kept.

### D2. One new message: `LinkState`
A full-state, origin-signed record, sent and relayed only over pinned, approved sessions:

```
LinkState {
  origin: NodeId, network: NetworkId,
  sequence: u64,            // per origin, strictly increasing across restarts (persisted)
  expires_in_seconds: u16,  // bounded, e.g. 60; refreshed every 20 s
  hosted_segment: Option<{ segment: SegmentId, prefix: IpPrefix }>,
  links: Vec<{ peer: NodeId, segment: SegmentId, health: LinkHealth }>,  // at most 32
  routing: bool,            // claim only; see D5
  signature: bytes,         // origin's control key over the canonical encoding
}
```

Full state (not deltas) makes it idempotent and lets a lost update heal on the next refresh.
Strict decoding, bounds and replay rules follow P7-002/P7-005.

### D3. Who said it: origin signatures
Each `LinkState` is signed with the origin's existing control TLS key (ECDSA P-256, whose SPKI
is already pinned for direct peers). A relayed record is accepted only if the signature verifies
against a **pinned key for that origin in this network** and the sequence is newer than the last
accepted one. Relays forward unchanged records and never re-sign. This closes F3 but needs F4
solved: every member must hold a pin for every other member (open question 1).

### D4. A link exists only when both ends say so
An edge A–B on segment S enters the graph only if A's current record lists B on S **and** B's
current record lists A on S, and S is a segment one of them hosts with a prefix that matches the
deterministic allocation for S (P8-002). A single member cannot invent a link to a node that
does not confirm it (threat T05). Colluding authenticated members can still lie together; the
threat model already accepts that insiders can misreport link quality.

### D5. Transit needs approval, not a claim
A node is used as a transit hop only if its own record claims `routing` **and** the local durable
approval marks it as a router for this network (a new per-peer flag set with PolicyKit, like
trust). Unknown or unflagged nodes are leaves. This applies T06: membership alone grants no
routing authority. (Open question 2.)

### D6. Expiry and loss
Records expire after `expires_in_seconds`; an expired origin's links leave the graph, and the
existing P8-008 path withdraws routes through it. A revoked peer's records are dropped at once,
and its pin removal makes its future records unverifiable.

### D7. Roles
Membership role becomes a set per network: at most one hosted segment plus zero or more client
attachments (schema v4 with migration from v3). Restart reconciliation (P6-009) and leave
(P6-008) then act per attachment. (Open question 3.)

## Required SPEC and threat-model changes (before code)
- SPEC: replace "the control protocol accepts no route/topology messages" with "it accepts only
  origin-signed, bilaterally confirmed link-state; routes are computed locally and never
  distributed".
- P14-001: add T05/T06 mitigations from D3 to D5; add signature, sequence-rollback and relay
  amplification (rate limit per origin) to the negative test list.
- P14-003: the deny-by-absence checkbox is superseded by "accept only `LinkState` meeting D3/D4".

## Staged plan (each stage its own commit with CI)
1. **S0** SPEC and threat-model edits above, reviewed.
2. **S1** `LinkState` type, canonical encoding, signing and verification in `blueroute-protocol`,
   with hostile tests (bad signature, unknown origin, replay, rollback, oversize, unknown fields).
   Still rejected on the wire.
3. **S2** pure graph builder: verified records → `TopologyGraph` with D4/D5/D6, table tests.
4. **S3** daemon: send and relay over existing sessions, build the graph, expose it read-only in
   diagnostics (`topology` already exists in `DiagnosticSnapshot`). No kernel mutation.
5. **S4** route application through the existing Linux executor, behind a default-off flag.
6. **S5** forwarding through the existing lease; then namespace/veth tests where CI allows, and
   the P8-006/007 hardware runbooks.

## Open questions for the maintainer
1. **Key distribution (F4):** how does every member get every other member's pin? Options:
   (a) the host vouches by distributing its clients' pins over the authenticated session,
   transitively trusted; (b) every pair enrolls out of band (does not scale); (c) a per-network
   signing key held by the network creator that certifies member keys. (a) is simplest; (c) is
   the cleanest trust model but adds key management.
2. **Transit authorization (D5):** require an explicit per-peer router approval, or accept any
   member whose capabilities claim routing?
3. **Role model (D7):** allow one node to host a segment and be a client of another in the same
   network (schema v4), or keep v1 to separate networks per star with no cross-network routing?
4. **Bilateral confirmation (D4):** acceptable as the v1 bar, given colluding insiders can still
   lie together?
5. **Timing:** build S0 to S3 now (software-only, behind the closed join gate), or wait until the
   first two-node join milestone exists on hardware?

## Maintainer answers (2026-11-12)

1. **Key distribution:** option (a). The host passes on its clients' pins over its authenticated
   sessions; trust chains through hosts. A per-network signing key (c) stays a possible later
   upgrade.
2. **Transit:** an explicit, PolicyKit-gated per-peer router approval is required; a routing claim
   alone never makes a node a transit hop.
3. **Roles:** allow one node to host a segment and be a client of another in the same network
   (schema v4), but only after the single-star role model has passed hardware acceptance.
4. **Bilateral confirmation:** accepted as the v1 bar.
5. **Timing:** do not build S0 to S5 yet. Wait until the first two-node single-star join has
   passed on hardware. Priority moves to the P7-003 enrollment flow and a deliberate, default-off
   way to enable join on a test rig, which unblock that milestone.

## Out of scope
Internet gateway and default routes (P18), IPv6 (P19), Babel or other routing daemons (P8-009),
automatic role selection (P9).

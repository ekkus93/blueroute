# ADR 0001 — Control-plane transport and authentication

- Status: Accepted (design chosen by the project owner on 2026-11-05: per-node Ed25519 identity keys)
- Task: P7-001
- Unblocks: P6-004, P7-002..P7-005, P8-005, P14-003, P18-003

## Context

`docs/SPEC.md` requires authenticated, versioned control communication separate from user traffic, and says source IP, Bluetooth names/addresses, and discovery metadata are not identity proof (see also `P6-004-JOIN-ORCHESTRATION.md`). Today:

- `NodeId` is 16 random bytes with no key material (`blueroute-linux/src/identity.rs`).
- Approval (P6-003) records "this `NodeId` may attempt to join network N", and `require_peer_approved(network, node)` is the fail-closed guard.
- Secrets already have restrictive Linux storage and a redacting wrapper (P3-005).
- The data plane (PANU/NAP, P6-005 addressing) gives a node an IPv4 address before any control session exists.

The decision needs to bind a control session to a stable BlueRoute identity and to network membership without trusting the transport.

## Decision

### Transport

- Control sessions are TCP connections over the established IP PAN. Not a Bluetooth side channel, not UDP.
- Default port `46100/tcp` (distinct from the P6-006 probe ports 46006/46007).
- The hosting node listens only on BlueRoute-owned segment addresses (bridge address), never on `0.0.0.0` or foreign interfaces.
- Address discovery uses the P6-005 plan: the host is `segment + 1`. For routed peers (P8), addresses come from authenticated topology facts (P7-003).
- A control session starts only after data-plane availability (P7-004).

### Identity

- Each node has a persisted Ed25519 identity keypair (private key in the P3-005 secret store, mode 0600, redacted `Debug`).
- `NodeId` is derived: the first 16 bytes of `SHA-256("blueroute/node-id/v1" || public_key)`. A `NodeId` therefore cannot be asserted without the key.
- Display names stay non-authoritative.

### Session handshake

- `Noise_XX_25519_ChaChaPoly_SHA256` via a maintained Noise implementation, giving mutual static-key exchange, forward secrecy, and an AEAD channel.
- The handshake payload carries the sender's Ed25519 public key plus a signature over `"blueroute/noise-binding/v1" || noise_static_public_key`. The verifier recomputes the `NodeId` from the Ed25519 key and checks the signature, binding the transport key to the BlueRoute identity.
- Source IP, MAC, and BlueZ `Trusted` are never consulted for identity.

### Membership and authorization

- A network has an *authority*: the Ed25519 key of the node that created it (the host).
- On join, the host checks `require_peer_approved(network, node)` (P6-003) before accepting. It then issues a *membership credential*: an Ed25519 signature by the authority over `("blueroute/credential/v1", network_id, node_id, not_before, not_after)`.
- Members verify credentials against the pinned authority key for that `NetworkId`. Non-members cannot obtain a credential, so they cannot send accepted topology or route messages (P14-003).
- The joining node pins the authority's `NodeId` from an explicit user/discovery action. Because the P6-002 advertisement is non-authoritative, the join UI must present the authority fingerprint for confirmation (P12-006). A rogue node advertising the same `NetworkId` is therefore caught at confirmation and cannot present a credential chain the client expects.
- Revocation: forgetting a peer (P6-003) stops reissue; credentials are short-lived and renewed over the live session. An authority-side deny list is enforced at handshake time.

### Freshness and replay

- Session messages are AEAD-protected with per-direction counters, which Noise provides, so captured traffic cannot be replayed into another session.
- The envelope (P7-002) carries a per-direction monotonic sequence number and the receiver rejects non-increasing values.
- Credentials are time-bounded. Nodes have no shared clock guarantee, so verification allows a bounded skew and the authority also timestamps renewals within a session.

## Consequences

- New dependencies: an Ed25519 implementation, a Noise implementation, and SHA-256. Exact versions are pinned and justified in the P7-002/P7-003 changes. `unsafe_code = "forbid"` still applies to BlueRoute code; dependency choice favors pure-Rust implementations.
- `NodeId` semantics change from random to key-derived. Legacy random identity files (development fixtures only) are not silently reused as authenticated identities: on first start the daemon keeps the legacy file untouched, generates a key, derives a new `NodeId`, and treats approvals recorded under the old ID as not applying. Dev fixtures re-approve peers. This is documented in the persistence change, with tests, and a future migration path stays possible because the identity file is versioned.
- The authority is a single node per network in v1. Authority loss means the network must be re-created. Authority transfer or multi-authority is out of scope.
- Residual risk: pinning relies on one explicit user confirmation. A user who confirms the wrong fingerprint joins the wrong network.

## Alternatives considered

- **Shared network secret / PSK.** Simpler, but any member can impersonate any other member, and a leaked secret compromises the whole network. Rejected for the malicious-member threat model (P14-001).
- **TLS with X.509.** Heavier certificate machinery and PKI semantics that do not fit a small local network. Rejected.
- **Source IP / Bluetooth identity.** Explicitly forbidden by SPEC.
- **Bluetooth-layer authentication only.** Pairing authenticates adapters, not BlueRoute identity or membership (established in P6-003).

## Follow-up tasks

| Task | Work |
| --- | --- |
| P7-002 | Envelope: version, sender/network, type, bounded payload, sequence. |
| P7-003 | Hello and capability exchange inside the authenticated session. |
| P7-004 | Session lifecycle and duplicate-session suppression. |
| P7-005 | Property/fuzz tests of the codec. |
| P14-003 | Membership verification on every topology/route message. |

## Implementation status

- Done (follow-up to P7-001): `blueroute-protocol::crypto` (Ed25519 identity, key-derived `NodeId`, Noise static-key binding signature, authority-signed time-bounded membership credentials with bounded clock skew) and `blueroute-linux::NodeKeyStore` (versioned `node-key` file, mode 0600, corrupt files rejected, legacy `node-id` never read). The daemon's `local_node` is now key-derived; an existing legacy `node-id` file is left in place and ignored, so dev fixtures must re-approve peers.
- Done: `blueroute-protocol::handshake`, a sans-I/O `Noise_XX_25519_ChaChaPoly_SHA256` handshake whose encrypted payloads carry the Ed25519 key and a Noise-key binding signature. It proves the peer `NodeId`, can require an expected peer, and yields an AEAD session that rejects replay and tampering. Dependency: `snow` 0.10.0 (default features).
- Done: `blueroute-linux::ControlChannel` carries the handshake and AEAD-protected P7-002 frames over any async stream (`u16` length-prefixed records, 10 s handshake deadline), tested over real loopback TCP: mutual auth, wrong-peer rejection, silent-peer timeout, garbage/early-close, and tamper rejection. Unauthenticated handshake message 1 must carry no payload.
- Done: `blueroute-linux::{serve_join, request_join}` and `blueroute-protocol::join`. The host checks approval after the handshake proves the peer, exchanges hellos, and issues a 1 h credential or denies. The joiner dials with the authority `NodeId` pinned (so a rogue advertiser fails the handshake), verifies the credential against the proven key with 5 min clock skew. Covered over real loopback TCP: approved, denied, rogue authority, expired window, and credential binding.
- Remaining: the daemon listener bound to the bridge address (46100/tcp) and session tasks that drive `SessionManager`, authority pinning and credential issuance on join (P6-004 production wiring), and a deny list.

# P7-003 — Peer hello and capability exchange

Implemented in `crates/blueroute-protocol/src/hello.rs`.

- `Hello` is the first (critical) control message of an authenticated session. It carries
  software version (informational), `ControlVersion`, the claimed `NodeId`, optional feature
  flags, `NodeCapabilities`, and the sender's PAN segment facts (segment id, host/client role,
  optional prefix).
- `Hello::from_envelope` rejects: a non-hello type, malformed payloads, out-of-bounds fields
  (software, features, segment count), and a claimed `NodeId` that differs from the envelope
  sender (which the P7-002 validator has already bound to the authenticated peer).
- `negotiate` requires the same major, picks the lower minor, intersects features, and carries
  the peer's capabilities and segments through. Decisions use capabilities only, never a
  computer model.

Not covered here: the Noise/Ed25519 session that authenticates the sender (ADR 0001, later
task) and the lifecycle that sends the hello (P7-004).

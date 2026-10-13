# P7-001 — Inter-node Control Transport (ADR)

**Date:** 2026-10-13
**Status:** Accepted architecture decision only; implementation pending P7-002..P7-004.

## Context

BlueRoute uses Linux IP over Bluetooth PAN (BNEP). The system daemon also needs an authenticated control channel for peer capabilities, network membership, topology and routes. BlueZ pairing, advertised IDs, IP addresses, and Bluetooth names cannot authenticate a BlueRoute NodeId. The production JoinNetwork gate must remain closed until this is implemented.

## Transport candidates and decision

- **TCP over BlueRoute-owned PAN IPv4, with mutual TLS 1.3 (selected):** reliable ordered byte stream over an existing link, standard secure libraries, explicit identity authorization, modest implementation complexity.
- **UDP/QUIC:** potentially viable but adds additional transport and handshake complexity for the first single-star stage.
- **RFCOMM/GATT control:** introduces a second Bluetooth transport whose discovery and lifecycle would have to be managed independently.
- **Plain TCP with MAC/IP/BlueZ-trusted/advertisement identity (rejected):** no cryptographic proof of the remote NodeId.
- **Single shared network password (rejected):** weak per-node identity binding and individual revocation.

## Locator and lifecycle

- Reserve TCP port **46071** for the initial daemon-to-daemon control endpoint. The initial NAP binds its *BlueRoute-owned host address* (the .1 address from the P6-005 deterministic /24), never 0.0.0.0 or an unrelated interface. The first PANU connects to that host address after PANU attachment and IP address assignment. Source address/port are locators, not authority.
- Future multi-segment endpoint advertisements must themselves come from authenticated network state, not unauthenticated device names. Address conflicts fail closed.
- Transaction ordering: preflight -> PANU -> IPv4 -> peer-authenticated TLS/session -> authorized membership commit. Reverse-order cleanup on failure. Do not accept unauthenticated join/route/gateway messages.
- Explicit, bounded timeouts for connect, TLS, framing, idle, reconnect and teardown; connection limits and duplicate-peer reconciliation.

## Peer identity and authorization requirements

- Each node requires a persistent asymmetric signing/authentication keypair in restricted secret storage. Use a widely audited TLS 1.3 implementation such as rustls, without inventing cryptography or permissive custom certificate verification.
- NodeId is a *separately generated logical identifier*, not itself a cryptographic public key. An approved (NetworkId, NodeId) must be pinned to a verified peer certificate/public-key fingerprint via a deliberate enrollment ceremony or equivalent independently authenticated proof. Check proof of possession, valid cryptography, pinned peer key, logical NodeId, and NetworkId on **both ends**.
- Existing P6-003 NodeId-only trust records have **no verified key binding**. They must not become live network authorization without explicit key enrollment/migration and reapproval. A changed key, revoked approval or conflicting identity fails closed.
- Bind framed messages to the **verified TLS channel**, intended NetworkId, NodeId and a fresh session nonce (for example via TLS exporter/transcript binding). Merely including identity fields in plaintext JSON proves nothing.
- Bluetooth MAC, display name, LE advertisements, IP source, BlueZ Trusted flag, self-reported hello and claimed capability never grant membership.
- Do not log private key material, TLS secrets, or credential-bearing transcripts. No fallback to unauthenticated control if TLS/authentication fails.

## Framing and future work

- P7-002 implements an independent versioned, bounded length-prefixed envelope with strict malformed/unknown-type rejection and replay sequencing. Parsing alone is **not authentication** and may not trigger network/route mutations.
- P7-003 adds peer hello, capability exchange and secure key enrollment/authorization binding.
- P7-004 adds authenticated TLS client/server connections, per-peer lifecycle, loss/reconnect, duplicate-session cleanup, timeouts and deterministic negative tests.
- P14 requires threat-model review and adversarial validation before remotely supplied topology/route state is accepted.
- All new physical PAN tests are deferred in HARDWARE_ACCEPTANCE_BACKLOG.md. Simulated peers, loopback integration and namespaces may qualify code but not real RF behavior.

## Open technical decisions

Finalize audited TLS library/certificate format, secure enrollment UX, key rotation and persistent trust-schema migration while implementing P7-003/004. The 46071 port is the v1 convention, not an identity claim.

**Acceptance:** IP address is never sole identity proof. Production join remains unavailable until cryptographic authorization is in place.

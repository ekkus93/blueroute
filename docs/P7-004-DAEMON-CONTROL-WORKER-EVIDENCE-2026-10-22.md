# P7-004 — Daemon-owned guarded control worker (software-only, 2026-10-22)

## Scope and exact-head qualification

- Source implementation: `1c37154704443969d73dc6676a5b864ec682a995`.
- Rust formatting: `f052ecb36f8205daab303c1ffd8a263a6eb0039f`.
- Rust ownership/compilation correction: `69075403ecb5643edc0a16a6fe6128309cdc8f59`.
- [Exact-head CI 39861673461](https://github.com/ekkus93/blueroute/actions/runs/39861673461): **passed**. Includes workspace formatting, locked check, Clippy, unit/integration and packaging gates.
- Task status: **IN PROGRESS / SOFTWARE PARTIAL**. Hardware acceptance **DEFERRED**.

## What is now integrated

The production daemon's `main.rs` launches a dedicated `blueroute-control-tls` OS thread after acquiring the local D-Bus service. The worker receives the persisted local TLS identity, peer pin/trust authorization service and the same `NetworkOperations` instance used for NAP host creation. This ensures synchronous certificate/TLS/Hello work does not block D-Bus dispatch.

The worker opportunistically calls `bind_guarded_control_listener`: no local active NAP means no TCP listener. It repeatedly re-observes durable host membership and the owned NetworkManager-derived control endpoint using `supervise_registered_from_durable_network_operations`. Failed ownership observations retire the listener and close all registered sessions; rebinds require a fresh guarded bind. The reusable authenticated accept path performs current durable pin approval, mutual TLS 1.3, exporter-bound Hello/HelloAck, registry admission and post-authentication ownership revalidation. The worker accepts no remote identity from a TCP source address and cannot invoke JoinNetwork.

The worker caps retained authenticated sessions at 32 and closes each session on its own 15-second monotonic time budget, independent of when other sessions were admitted. An expired or regressed lease retires only its exact registered TLS generation. An additional deterministic unit test verifies that expiry of one peer's provisional lease does not shorten a newer peer's lease, and that regressed time is rejected. This **is not yet** a full liveness protocol: the daemon does not consume Heartbeat/Goodbye frames, maintain per-peer active deadlines, or route authorized control messages over these sessions. The interval is provisional fail-closed protection against indefinite session ownership, not a claim of usable stable peer connectivity.

## Remaining required P7/P6 work

1. Design and wire independently user-confirmed SPKI enrollment end to end through a permission-gated daemon/CLI or desktop ceremony, with explicit negative-authentication and revocation tests. NodeId trust alone is insufficient.
2. Replace the provisional global short session lifetime with a continuous per-peer registered transport supervisor: read and validate framed authenticated Heartbeat/Goodbye, handle EOF, idle expiration, cancellation and link-loss events, and maintain bounded reconnect state. Prove concurrency and recovery using real localhost mutual TLS and fake network backends.
3. Add production PANU discovery/connect and authenticated request/response behavior for P6-004 JoinNetwork. Preserve reverse-order rollback and deny all membership changes before verification.
4. Test a running packaged daemon's actual worker under simulated owned NAP/interface transitions, certificate revocation, D-Bus request concurrency and restart. Passing existing library tests is not equivalent to this test.
5. Complete deferred two-node Bluetooth PAN/TLS acceptance using the hardware evidence requirements in `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`. Do not mark P7-004 or P6-004 complete from CI alone.

## Boundaries

This evidence qualifies source and CI only. It is **not** proof that a physical NAP binds, another node completes an authenticated handshake, application traffic traverses Bluetooth, or daemon JoinNetwork is functional. Do not mark physical criteria complete.

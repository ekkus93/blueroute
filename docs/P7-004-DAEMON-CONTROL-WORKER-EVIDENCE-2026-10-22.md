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

**Historical provisional lease implementation:** the original worker capped authenticated sessions at 32 and retired each session after its own 15-second monotonic lease. That short lease and its best-effort expiry Goodbye were interim fail-closed protections, superseded by the later active Heartbeat/Goodbye worker described below. Neither version enables production JoinNetwork or demonstrates radio transport.

## Bounded authenticated expiry teardown follow-up (2026-10-22)

On a *known* elapsed 15-second provisional lease, the daemon worker now attempts an authenticated, best-effort Goodbye with a 100 ms absolute write budget before dropping that registered TLS owner. The writer rechecks durable peer approval; even on a failed or timed-out write, dropping the owner closes its TCP socket and retires only the exact registry generation. Missing lease metadata and regressed clocks close silently rather than sending traffic. This is an orderly-teardown improvement, **not** active inbound Heartbeat/Goodbye processing or liveness qualification.

- Source: `4e6d4e03fff7c2d5795c3b5184696257802d7a60`; formatting/compilation/Clippy corrections through `a2d43ca6cd90b4d345367325a2a165b5c6ebbac5`.
- [Exact-head CI 39864751716](https://github.com/ekkus93/blueroute/actions/runs/39864751716): **passed** (locked Rust formatting, workspace check, Clippy and tests).
- P7-004 and P6-004 remain **IN PROGRESS**, physical acceptance **DEFERRED**.

## Active per-peer Heartbeat/Goodbye and timeout regression (2026-10-22)

The daemon worker now polls each registered, mutually authenticated TLS session for replay-checked inbound control frames with a bounded 10 ms socket-read slice per peer. Valid inbound Heartbeat frames renew that peer's 30-second idle deadline; an empty slice does not. Inbound Goodbye, clean EOF, authorization failure, expired idle deadlines and transport failures retire only the corresponding registered session. The worker also emits authenticated outbound Heartbeat frames every 5 seconds with a 100 ms write budget and best-effort terminal Goodbye on peer-requested close. All sends recheck durable approval. A 32-session cap and fresh guarded NAP ownership checks remain in place. These are source-level behaviors; no live PAN acceptance or production join is implied.

- Worker implementation is present at `469b8f5545163aa0a95e275bb45f52373ea93641` (formatting commit for the bounded supervision worker).
- The prior exact-head [CI 39870661020](https://github.com/ekkus93/blueroute/actions/runs/39870661020) **failed** one timing-sensitive real localhost TLS regression: a full idle-budget socket timeout returned a nonterminal result due to submillisecond elapsed-time rounding.
- Commit `acd96713ac9ff230b9f6270bb3549e620b29aefc` makes a full-idle-budget `TimedOut`/`WouldBlock` terminal and fail-closed even when the OS wakes slightly before the requested timeout; short bounded read slices remain nonterminal. [Exact-head CI 39871306504](https://github.com/ekkus93/blueroute/actions/runs/39871306504) **passed** on this revision, including locked Rust formatting, workspace checks, Clippy, unit and integration tests.
- **Still open:** running-daemon multi-peer, revocation, NAP ownership and restart fault-injection qualification; independently confirmed enrollment UX; PANU-side authenticated session initiation and JoinNetwork; physical two-node Bluetooth acceptance. Do not mark P7-004/P6-004 complete.

## Remaining required P7/P6 work

1. Design and wire independently user-confirmed SPKI enrollment end to end through a permission-gated daemon/CLI or desktop ceremony, with explicit negative-authentication and revocation tests. NodeId trust alone is insufficient.
2. Qualify the newly integrated per-peer authenticated Heartbeat/Goodbye supervisor under running-daemon concurrency, revocation, cancellation, link loss, stale-generation callbacks, reconnect and restart; add bounded reconnect execution and fault-injection coverage. Current worker behavior and source-only unit/localhost tests are not complete daemon lifecycle acceptance.
3. Add production PANU discovery/connect and authenticated request/response behavior for P6-004 JoinNetwork. Preserve reverse-order rollback and deny all membership changes before verification.
4. Test a running packaged daemon's actual worker under simulated owned NAP/interface transitions, certificate revocation, D-Bus request concurrency and restart. Passing existing library tests is not equivalent to this test.
5. Complete deferred two-node Bluetooth PAN/TLS acceptance using the hardware evidence requirements in `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`. Do not mark P7-004 or P6-004 complete from CI alone.

## Boundaries

This evidence qualifies source and CI only. It is **not** proof that a physical NAP binds, another node completes an authenticated handshake, application traffic traverses Bluetooth, or daemon JoinNetwork is functional. Do not mark physical criteria complete.

## Cancellation-aware daemon worker ownership (2026-10-22)

The daemon now retains a `DaemonControlWorker` owner instead of a bare detached `JoinHandle`. Dropping it sets a release-ordered atomic stop request and unparks the dedicated TLS thread without synchronously blocking the D-Bus executor. An explicit `shutdown()` method requests stop and joins the worker. Both the 250 ms supervision interval and the 2-second rebind interval are interruptible parks; the loop checks cancellation before session I/O and new inbound TLS admission. When the loop exits, its registered session vector and owned listener are dropped, closing exact-generation sockets and releasing the listener. Two deterministic thread tests verify wakeable shutdown and nonjoining RAII drop.

- Source: `89ca005cc73f02161703fb4d7b0385c95f207c77`; rustfmt correction: `5a2a9a2885cb55e9f111d569c7f8f40869f11ee2`.
- [Exact-head CI 39877640079](https://github.com/ekkus93/blueroute/actions/runs/39877640079) **passed** (formatting, locked Rust checks, Clippy, tests and integration gates).
- Limitation: already-running TLS handshake or backend observation may delay cancellation; no real daemon restart, PAN detach, physical radio or production JoinNetwork acceptance is claimed.

## Actual idle worker lifecycle regression (2026-10-22)

A fake `NetworkOperations` implementation returns no active NAP while counting real guarded bind attempts. The test starts `spawn_daemon_control_worker` with a generated local TLS identity and durable trust service, waits until the actual worker has entered its no-NAP rebind path, and asserts `shutdown()` wakes and joins within one second rather than waiting the two-second rebind interval. This qualifies actual thread cancellation in the idle/no-NAP path, not a running system-D-Bus daemon, active mutual-TLS teardown or a physical PAN.

- Test source `83057c125d6a8360389ea782a7bf7fec3a26da70`; formatting correction `b23136286aaf42175f16233ee93ee21251871e81`.
- [Exact-head CI 39878309790](https://github.com/ekkus93/blueroute/actions/runs/39878309790) **passed**.

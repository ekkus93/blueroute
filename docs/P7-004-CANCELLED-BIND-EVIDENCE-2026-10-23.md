# P7-004 — Cancelled NAP bind cleanup (software-only)

**Date:** 2026-10-23 UTC  
**Repository:** `ekkus93/blueroute`  
**Implementation commit:** `8265fd48e9f711ff111155a6d4c4b7d5612971a4`  
**Exact-head CI:** [39919568152](https://github.com/ekkus93/blueroute/actions/runs/39919568152) — **passed**.

## Failure mode

The live daemon control worker can await an independently guarded backend NAP bind. If shutdown was requested *during* that pending bind, an old worker could return from the backend and continue to reload durable membership and enter the authenticated control supervision path before the next cancellation check. The previously returned TCP listener would close when the worker ultimately exited, but no post-cancel observation or admission should be attempted.

## Fix

`apps/daemon/src/control_worker.rs` now stores the bind result, immediately checks its atomic stop request, and leaves the worker loop before inspecting the bound listener. Dropping the unused result closes its owned TCP socket. This does not bypass the existing durable-membership and guarded endpoint checks for ordinary, uncancelled binds.

## Deterministic regression

`cancellation_during_nap_bind_closes_socket_before_any_observation` constructs a real localhost `TcpListener` via a fake `NetworkOperations` backend. The backend pauses after opening the socket and before returning its `ObservedControlListener` owner. The test then:

1. Confirms that the already-bound socket cannot be rebound while the fake backend owns it.
2. Requests worker cancellation deterministically while the bind is still pending.
3. Releases the bind gate and joins the daemon worker.
4. Confirms exactly one bind attempt, **zero** post-cancellation `observed_owned_control_endpoint` calls, and release of the original listener so that its address can be rebound.

The run also passed repository formatting, locked compile, Clippy, Rust tests and pre-existing daemon D-Bus integration/packaging checks.

## Remaining acceptance

- This tests the daemon worker and a fake backend on local TCP; it does **not** validate BlueZ/BNEP/PAN, any Bluetooth radio or physical link.
- Cancellation does **not** forcibly interrupt an already-blocking backend call or mutual-TLS handshake. Such operations remain subject to their existing bounded deadlines and backend behavior.
- Independent human-confirmed key enrollment (P7-003), complete authenticated inbound/outbound PANU session/reconnect lifecycle (P7-004), and production JoinNetwork (P6-004) remain open. Physical acceptance is **HARDWARE DEFERRED**, not passed.

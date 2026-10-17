# P8-005 — Linux route executor software qualification

**Date:** 2026-10-17  
**Source commit:** `8d588909e3e2ffe9821c2d1e1702731da607640b`  
**Exact-head CI:** [39055021582](https://github.com/ekkus93/blueroute/actions/runs/39055021582) — passed.

## Implementation

`crates/blueroute-linux/src/route_executor.rs` exposes `LinuxOwnedRouteOperator`, `LinuxRouteChange`, `LinuxRouteExecutionReport`, and `execute_linux_route_reconciliation`.

The software-only execution layer computes an owner-scoped desired/observed plan, obtains fresh **full-table, foreign-inclusive** observations before each operation and afterward, enforces exact expected-result snapshots, and records confirmed changes only. Upserts precede stale route removals. A concurrent observation mismatch or a failed backend comparison stops the executor; an ambiguous partially applied write must be reobserved and replanned rather than optimistically rolled back.

Seven deterministic fake-kernel tests cover replacement/removal/idempotence, unrelated foreign routes, foreign-destination collision before mutation, pre-write observation races, privileged compare-and-swap rejection, partial-write replan, success-without-effect detection, and changes after partial progress. The test suite, rustfmt, locked workspace build, Clippy with warnings denied, D-Bus and packaging integration gates passed at the stated exact SHA.

## Still open

This is a **trait and tested fake backend**, not a Linux kernel route writer. A production `LinuxOwnedRouteOperator` must obtain authoritative complete kernel state including unmanaged routes, fail closed when ownership is unprovable, and implement atomic expected previous-state/owner/interface/table checks at the actual privileged mutation site. `IpNetworkBackend::routes` only exposes owned state and is not suitable as the executor's complete observation. Daemon-per-network serialization, independently authenticated peer/PAN binding provenance, revocation, restart, network namespace/veth testing, real Bluetooth multi-hop and hardware acceptance all remain unqualified. Do not infer any physical or actual privileged route change from the fake tests.

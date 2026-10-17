# P9-003 / P9-004 software reconciliation evidence (2026-10-16)

## P9-003 — Pure topology plan diff

`crates/blueroute-core/src/topology_plan.rs` computes an ordered difference
between observed and desired NAP links. It preserves unchanged links, prefers
make-before-break for independent connections, explicitly identifies
break-before-make client migrations, and enforces affirmative desired-host
capacity and reciprocal neighbor capability. Five deterministic tests cover
unchanged plans, independent replacement, migration, full capacity, and invalid
assignments.

The initial implementation at `9fb4e592` failed formatting. Follow-up
formatting fixes at `fa3cfd3c` and `8e2d1e35` restored passing exact-head
CI: https://github.com/ekkus93/blueroute/actions/runs/38998276234 .

## P9-004 — Observation-gated execution contract (software partial)

`crates/blueroute-core/src/topology_executor.rs` defines a backend-neutral
`NapTopologyOperator` and `execute_nap_plan`. The executor rejects stale
initial observations, serializes ordered link intents, reobserves after every
backend operation, reconciles routes against observed links, stops immediately
on failures, and requires re-observation/replanning before retries.

`crates/blueroute-core/tests/topology_executor.rs` uses an injected fake
backend to verify make-before-break, route ordering, stale-observation
rejection, and failure preserving the existing link.

Exact-head CI at `da0947b8a7e3e15183541f50db4068f2b8fc0410` passed:
https://github.com/ekkus93/blueroute/actions/runs/38998619932 .

## Unmet acceptance

These are **software-only** components. No production BlueZ PAN link operator,
authenticated peer membership gate, Linux route-reconciliation adapter, daemon
lifecycle coordinator, or physical Bluetooth recovery has been connected.
P9-004 must remain **IN PROGRESS**, and P9-005 hardware acceptance remains
**DEFERRED**. P7-003/P7-004 authentication and P8 route integration remain
dependencies. Do not claim successful live topology changes from pure planner
or fake-backend tests.

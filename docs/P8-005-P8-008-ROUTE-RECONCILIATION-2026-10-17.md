# P8-005/P8-008 — Observed routing and simulated failover evidence

**Date:** 2026-10-17  
**Scope:** deterministic software only; no live Bluetooth or kernel routing qualification

## Implementation

- `crates/blueroute-core/src/route_executor.rs`: backend-neutral `OwnedRouteOperator`, `RouteChange`, `RouteExecutionReport`, and `execute_route_reconciliation`. The backend contract requires atomic compare-and-swap of exact existing route/owner, while the executor reobserves before and after every operation. It preserves foreign routes, rejects unexpected concurrent changes, upserts before removing stale routes, and stops without fictitious rollback after partial failure.
- `crates/blueroute-core/src/route_recovery.rs`: `reconcile_observed_topology_routes` combines desired routes computed from active observed topology with the gated executor. Simulated tests verify primary failure, alternate next hop, partition withdrawal, path restoration, foreign-route preservation, and partial-write convergence.

## Qualification

- P8-005 implementation `5da26e4861cbcfcc502bd308b5ac84f94ea4dafe` — [CI 39021539274](https://github.com/ekkus93/blueroute/actions/runs/39021539274), passed.
- P8-008 software recovery `252eb27db6b1da479e9664881384942a2d3f2c32` — [CI 39021827816](https://github.com/ekkus93/blueroute/actions/runs/39021827816), passed.

CI covered formatting, locked workspace, Clippy, tests and standard repository integration gates. Earlier format/lint failures were fixed, not waived.

## Additional P8-005 destination-prefix hardening

`route_reconciliation.rs` now rejects noncanonical IPv4 and IPv6 prefix intents (host bits set) so a future kernel backend cannot accidentally treat two textual encodings of one route destination as different owned records. Canonical prefixes are accepted. [CI 39022369709](https://github.com/ekkus93/blueroute/actions/runs/39022369709) passed at `3baa6030d8035e6d079ed6615873eb8ea21de49e`. The kernel backend must still independently canonicalize, match and enforce exact route ownership.

## Observed-prefix ownership alias regression (2026-10-17)

The earlier desired-prefix check was insufficient for noncanonical *observed*
foreign routes: a foreign `10.20.0.77/24` could evade exact comparison with
a desired `10.20.0.0/24`, despite representing the same kernel destination.
`plan_route_reconciliation` now canonicalizes observed destination keys
before checking conflicts, rejects malformed owned observations, and preserves
unrelated foreign routes. Regression tests cover IPv4 and IPv6 aliases and
malformed owned prefix observations. Qualified on exact `master` SHA
`24153e0674f2249cdab80350c14d6ba9897dd770`:
[CI 39045842735](https://github.com/ekkus93/blueroute/actions/runs/39045842735)
passed. This remains software-only evidence; the privileged Linux backend must
perform its own atomic kernel-side ownership comparison.

## Not yet accepted

Neither module authenticates peers, owns a Linux route table, mutates kernel routes or knows whether remote topology is trustworthy. The privileged backend must independently enforce atomic ownership checks at the mutation site and map verified logical routes to owned local interfaces and nondefault IP prefixes. Daemon integration must serialize per-network operations, confirm fresh verified PAN/session/membership observations, handle revocation and service restart, and preserve unrelated routing state. Linux namespace/veth, multi-hop and real Bluetooth acceptance remain outstanding. P8-005/P8-008 stay open.

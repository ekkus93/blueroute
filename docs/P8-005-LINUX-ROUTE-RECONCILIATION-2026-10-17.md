# P8-005 — Linux route reconciliation planning (software only)

**Date:** 2026-10-17  
**Qualified code head:** `c6e0c9f85089577a179e8abb82641e19d2fb37be`  
**CI:** [Run 39051662010](https://github.com/ekkus93/blueroute/actions/runs/39051662010) — passed

## Implementation and verified properties

- `crates/blueroute-linux/src/route_reconciliation.rs` exports `plan_linux_route_reconciliation` and `LinuxRouteReconciliationPlan`.
- Deterministically returns `upsert` and `remove` intents for a single nonzero `NetworkId`, and yields an empty plan after convergence.
- Checks canonical RFC1918 desired destinations, valid private next hops, positive route metrics and network-scoped ownership.
- Canonicalizes observed prefix keys before comparing against wanted destinations; a foreign noncanonical alias of a desired kernel route fails closed.
- Rejects duplicated desired/owned observed destinations and suspicious malformed owned observations. Other networks' routes are not included in removal intents.
- Five deterministic regression tests cover replacement, idempotence, foreign aliases, unrelated foreign routes, ambiguous/invalid ownership and unsafe gateway/default/self-route inputs.
- Exact-head GitHub Actions passed Rust formatting, locked workspace compilation, Clippy with denied warnings, workspace tests and repository integration/package validation.

## Stale-route foreign-alias collision hardening

Owned routes scheduled for removal must not overlap a separately observed foreign route at the same *effective* kernel destination. The planner now records canonical foreign destination keys and rejects an existing owned/foreign collision even when that destination is absent from the desired route set. A regression checks both observation orders for `10.20.0.0/24` versus `10.20.0.77/24`. [Exact-head CI 39052205507](https://github.com/ekkus93/blueroute/actions/runs/39052205507) passed at `23e4f17af92bfdb884d2f502c91b264a3db9352d` (format, locked build, Clippy, tests and repository checks). This is software safety evidence only; no kernel mutation was exercised.

## Unqualified and missing

This module is a **pure planner**, not a kernel route writer, full FIB inspection, validated provenance of observed ownership tags or atomic route mutation. Existing `IpNetworkBackend::routes` returns BlueRoute-owned routes only; production planning must obtain complete authoritative kernel observations (including unowned/foreign routes) before using this planner. A privileged backend must enforce exact destination/owner/table/interface comparison at mutation time and reject concurrent changes. Independently verified PAN/member bindings, authenticated route distribution, daemon lifecycle integration, network namespace/veth tests and actual Bluetooth multi-hop acceptance remain open. No live route-table changes, physical peer traffic or hardware conclusions are claimed here.

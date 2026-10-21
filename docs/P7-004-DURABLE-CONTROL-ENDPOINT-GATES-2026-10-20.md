# P7-004 — Durable control-endpoint and raw-bind gates (software only)

**Date:** 2026-10-20 UTC (2026-10-20 PDT).  
**Scope:** `SingleStarNetworkOperations` control endpoint observation and legacy raw TCP listener binding.  
**Repository:** `ekkus93/blueroute`; direct commits on `master`.

## Implemented

1. `observed_owned_control_endpoint` now requires a single current durable local member before calling the runtime, reloads durable membership after the asynchronous runtime observation, and rejects any network change or disappearance. A reported endpoint must equal the deterministic host IPv4 and reserved P7 TCP port for that durable network; no endpoint is exposed on mismatch.
2. `bind_owned_control_listener`, the legacy raw TCP listener path, now applies the same before/after durable membership check and verifies the bound socket's exact planned network endpoint. The owned socket is dropped when the check fails, so a stale raw listener is not returned. The guarded listener remains the required production API.
3. Deterministic fake-runtime regressions cover no durable member (zero runtime observations/binds), a valid endpoint, wrong endpoint, durable network replacement during the awaited observation/bind, and closure of the rejected raw TCP listener. These tests do not rely on a Bluetooth controller.

## Exact-head qualification

- Implementation `64112abbe4cd86f8686ee1185e3f8e659b3b8f8e`: [CI 39520020136](https://github.com/ekkus93/blueroute/actions/runs/39520020136) **passed**.
- Raw bind hardening `fa43d3b2a03aaf36248710e3b299d08fea06e28d`: [CI 39520292154](https://github.com/ekkus93/blueroute/actions/runs/39520292154) **passed**. The Rust CI job completed successfully at each exact SHA.

## Limits and remaining acceptance

The check is a snapshot guard, not a durable lease. The daemon must continue to re-observe host ownership, membership, and approval before TCP acceptance and authenticated-session activation, and must terminate live sessions on loss. No daemon PAN accept loop, independent user-confirmed SPKI enrollment, authenticated production `JoinNetwork`, cross-machine Bluetooth PAN traffic, or physical acceptance is established here. **P7-003/P7-004/P6-004 remain IN PROGRESS; hardware acceptance remains DEFERRED.**

The authoritative `docs/TODO.md` must ultimately link this evidence. A prior full-body TODO update was denied by platform safety checks; this separate evidence record does not assert that the large TODO file has been changed.

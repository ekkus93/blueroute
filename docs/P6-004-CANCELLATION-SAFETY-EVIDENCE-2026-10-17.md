# P6-004 — Canceled join rollback safety (software-only)

**Date:** 2026-10-17 (Pacific)  
**Qualified source head:** `ea171f65c655ff448992bac0a171a4c0aa40ff78`  
**Exact-head CI:** https://github.com/ekkus93/blueroute/actions/runs/39103583819 — passed

## Problem and fix

An async JoinNetwork future could be dropped after PANU activation or during
reverse-order rollback. Previously its drop guard unconditionally released
the coordinator, allowing another join to mutate the same network resources
even when cleanup was canceled or had failed.

`JoinTransitionGuard` now distinguishes safe pre-mutation cancellation
from potentially live runtime resources. PANU activation marks the coordinator
as requiring reconciliation. Successful reverse-order cleanup or a durable
membership commit clears the marker; canceled futures and incomplete cleanup
leave it fail-closed, rejecting subsequent joins with `InvalidState`.
The guard remains scoped to the same daemon coordinator instance.

## Qualification

The fake-runtime regression
`canceled_join_during_rollback_blocks_reuse_until_reconciliation`
suspends address cleanup after an injected control-authentication failure,
drops the pending future, and verifies that same-network and
different-network retries cannot start another PANU/IP mutation or commit
membership. Existing preflight-cancellation and successful-rollback tests
continue to cover release of a safely held coordinator.

The exact source SHA above passed the repository's GitHub Actions CI.

## Boundaries and remaining work

This is a conservative safety barrier, **not** asynchronous cleanup on
cancellation, persistent restart recovery, live PANU/NetworkManager ownership
reconciliation, independently approved mutual-TLS enrollment, production
JoinNetwork, or physical Bluetooth evidence. A new coordinator created after
daemon restart still requires P6-009 runtime reconciliation before join can
be safely enabled. P6-004 remains IN PROGRESS; do not check its physical
acceptance boxes or remove its fail-closed production preflight.

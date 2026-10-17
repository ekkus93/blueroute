# P9-005/P9-006 — Confirmed hub-loss recovery retry qualification

**Date:** 2026-10-17  
**Scope:** deterministic software/fake-PAN qualification only  
**Implementation:** `67bb55475b94824c552444f8ec68783cd6857b14`  
**Formatting repair:** `2e2876517dc4570a2f2303747351ca8919f8a0f9`  
**Exact-head CI:** [39013976520](https://github.com/ekkus93/blueroute/actions/runs/39013976520) — passed

## Implementation

The core library now exports `HubRecoveryRetryState` and `execute_confirmed_hub_loss_recovery_with_backoff`. The opt-in retry path samples authoritative PAN observations through `HubLossTracker` on each call, including deferred attempts. It uses the P9-006 stability policy's bounded exponential failure backoff when P9-005 recovery fails. Deferred attempts neither mutate PAN links nor reconcile routes.

On an eligible retry, the original recovery path enforces stale-snapshot validation, observed-link routing reconciliation, and observation-gated link execution. A successful execution resets retry history. If the failed hub reappears, retry history is cleared and any later unrelated loss begins from a fresh confirmation interval.

## Tested

- Failure after a partial replacement connection is not silently treated as completed recovery.
- While a retry is deferred, PAN observation continues and no route or link mutation occurs.
- The next eligible attempt replans from actual partial state and completes the missing connection.
- Recovery success clears failures; original hub restoration also clears failures.
- Repeated observation at the same timestamp cannot advance retry state.
- An invalid anti-flap policy is rejected before use.

The CI run includes Rust formatting, locked workspace compile/Clippy/tests and the repository's integration/package gates.

## Outstanding

- Wire the retry state to a **single serialized per-network daemon owner** and to authenticated observed BlueZ PAN links. A fake backend, untrusted advertisement, or claimed control message must not authorize a recovery action.
- Integrate daemon event dispatch, cancellation, and route reconciliation with owned Linux connections; qualify repeated hub loss, partial failure, and restart through software fault injection.
- Keep P7-003/P7-004 authenticated enrollment/session prerequisites and P6-004 production join fail-closed.
- Conduct representative multi-device Bluetooth recovery acceptance later under `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`. No RF/range/throughput or PAN claims are supported by these tests.

# P9-005: Confirmed hub-loss recovery execution (software-only)

Date: 2026-10-16

The core now exposes `execute_confirmed_hub_loss_recovery`, composing independently observed PAN links, `HubLossTracker` confirmation, `plan_hub_loss_recovery`, stale-route reconciliation, and the observation-gated `execute_nap_plan` backend contract.

Deterministic fake-backend regressions cover (1) debounced hub loss and replacement links with route repair, (2) a partial connection failure followed by a fresh observed-state replan and successful retry, and (3) intentional hub removal without recovery. Repeated loss notifications do not create duplicate connections.

**Safety boundaries:** This is a backend-neutral software orchestration seam. Callers must serialize it per network and provide independently observed, authorized PAN links and authenticated capability inputs. It does not subscribe to BlueZ, establish mutual TLS, independently enroll peer keys, implement production JoinNetwork, or prove Bluetooth radio failover. Physical P9-005 acceptance and live daemon integration remain open. `docs/TODO.md` remains authoritative and must not be marked complete on these tests alone.

Qualification: pending exact-head GitHub Actions CI.

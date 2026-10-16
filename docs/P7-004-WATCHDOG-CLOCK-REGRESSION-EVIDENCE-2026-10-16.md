# P7-004 Watchdog Clock-Regression Teardown — Software Evidence

**Date (UTC):** 2026-10-16
**Repository:** ekkus93/blueroute
**Code commit:** `ce593ca07e4e70ee3811f0df41aaa35977cd67cf`
**Exact-head CI:** https://github.com/ekkus93/blueroute/actions/runs/38878330621 (passed)
**CI job:** Rust, job 116898683505 (passed)

## Defect and remediation

`PendingHandshakeSockets::watchdog` previously propagated `ReconnectScheduleError::ClockRegression` from `ReconnectScheduler::take_expired_attempt` before closing its owned unauthenticated TCP socket. That could leave a connected pending socket and an in-flight scheduler generation after a rejected timestamp.

The code now handles the scheduler error explicitly, first shutting down and dropping only the socket whose (NetworkId, NodeId, generation) matches the attempted callback, then removing only the matching retry slot. It preserves the original error, does not implicitly schedule another retry, and leaves other peers or newer generations unaffected.

## Deterministic qualification

`regressed_watchdog_clock_closes_socket_and_retires_only_matching_generation` asserts actual localhost TCP EOF, no retained socket for the failed attempt, no later retry for that cancelled generation, independent activity on another peer, and a fresh generation that survives a stale watchdog callback.

Run #38878330621 completed Rust formatting, locked workspace build/check, Clippy, tests, simulated P6-006 socket contracts, D-Bus integration and authorization packaging checks.

## Acceptance boundary

This is a **software-only pending-handshake transport safeguard**. The production daemon still does not wire all outbound/inbound sockets into a fully authenticated runtime, and independent user-verified key enrollment, live TLS peer sessions, P6-004 JoinNetwork and final physical Bluetooth/multi-computer acceptance remain OPEN. No physical evidence or v1 release is claimed.

## Operational notes

Ralph Bridge 0.6.2 (build `0acea4b23362e2ccecab8f6c9db4418b162426cf`), server instance `2b818998d9ea4beab846a083bc4b3224`, GitHub App auth, accepted source operation `10acb8bb56d04b4bbc2eab3e70391170`. The first local patch reconstruction attempt was aborted before writing due to a client-side context typo; the accepted second attempt used exact-head compare-and-swap. CI polling initially timed out without failure; later exact-run status returned success. No secrets captured.

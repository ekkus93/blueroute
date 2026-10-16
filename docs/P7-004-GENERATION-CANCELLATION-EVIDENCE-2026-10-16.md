# P7-004 — Exporter-generation-scoped cancellation (2026-10-16)

**Status:** SOFTWARE PARTIAL. P7-004 remains IN PROGRESS; physical acceptance is HARDWARE DEFERRED.

The registered, mutually authenticated TLS session owner now offers
`cancel_if_current(expected_session_id)`. A stale PAN loss or reconnect
callback cannot close the currently owned TLS socket or retire its registry
slot unless its exporter-derived session ID matches. Matching callbacks
close the socket before retiring the registry entry; repeated cancellation
remains idempotent. This does not change the existing unconditional
`cancel()` API, which remains suitable for explicit owner shutdown.

**Implementation:** `53ca156d6398c6e2d14814c03adc11e0ab4933ea`.
**Qualified head:** `ab251cee37e837c549ed41b97944ed9ab24701b5`.
**Exact-head CI:** run 38947315578, passed (format, check, Clippy,
workspace tests and existing localhost mutual-TLS regressions).

The `registered_generation` integration target confirms the API is
callable from the daemon crate. A dedicated two-party, two-generation localhost
mutual-TLS regression in `registered_terminal_teardown.rs` now also verifies:
a mismatched callback leaves the first channel alive; matching cancellation
closes and retires it; a new independently authenticated TLS connection
receives a different exporter session ID while the old Rust owner is alive;
a delayed callback carrying the old ID cannot cancel the new connection or
retire its registry slot; the replacement sends a valid heartbeat; and
matching cancellation terminates the replacement. This closes the previously
unqualified local stale-generation regression, not live daemon integration.

**Regression commit:** `c73a0b2292db46b70e17b25838b16f738e4f052d`.
**Exact-head CI:** [38953834551](https://github.com/ekkus93/blueroute/actions/runs/38953834551) passed (format, locked check, Clippy, workspace tests, and integration coverage).

**Still open:** connect the guard to actual daemon PAN/link and reconnect
callbacks; exercise stale-vs-current callbacks against a replacement live
mutual-TLS channel; independently approved key enrollment; production
JoinNetwork; physical two-device Bluetooth acceptance. No radio, PAN,
routing, or production membership behavior is proven here.

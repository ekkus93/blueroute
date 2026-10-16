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

The new `registered_generation` integration target currently verifies
that the generation-scoped API is callable from the daemon crate. A proposed
dedicated two-party localhost stale-callback regression was not retained:
its first version failed rustfmt, and subsequent revisions could not be
qualified. **Do not count this as completed stale-generation runtime
acceptance.** The existing authenticated cancellation regression continues
to exercise unconditional `cancel()` over localhost TLS.

**Still open:** connect the guard to actual daemon PAN/link and reconnect
callbacks; exercise stale-vs-current callbacks against a replacement live
mutual-TLS channel; independently approved key enrollment; production
JoinNetwork; physical two-device Bluetooth acceptance. No radio, PAN,
routing, or production membership behavior is proven here.

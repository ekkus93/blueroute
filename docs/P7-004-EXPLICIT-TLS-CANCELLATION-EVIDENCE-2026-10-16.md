# P7-004 — Explicit registered TLS cancellation evidence (2026-10-16)

- **Implementation:** `40e6735131a65a4de93f8aa5fedab43d1d0dade3`
- **Exact-head CI:** https://github.com/ekkus93/blueroute/actions/runs/38938827311 — passed.
- **Change:** `RegisteredControlSession::cancel()` closes the owned authenticated TCP/TLS socket before retiring its exact session-registry generation. Repeated cancellation is idempotent.
- **Regression:** A real localhost two-peer mutual-TLS test confirms that cancellation retires the registry entry before owner destruction, and that the remote peer observes transport closure while the cancelled owner remains alive.
- **Limits:** This method is not yet called by a running daemon/PAN transport. Independent operator-confirmed enrollment, reconnect lifecycle wiring, production JoinNetwork and physical Bluetooth acceptance remain open. Do not treat this as physical PAN evidence.

P7-003, P7-004 and P6-004 remain IN PROGRESS; physical acceptance remains HARDWARE DEFERRED.

# P7-004 — Corrupt durable host membership regression plan

**Status:** Planned software regression, not completed acceptance. Physical Bluetooth testing remains deferred.

## Security invariant

An independently observed NAP must not authorize an inbound control listener when the owner-only durable membership store is malformed, has an unsupported schema, or cannot be read. No automatic repair or fallback to backend-only ownership is permitted.

## Deterministic daemon-worker test

1. Create a valid persisted local-host network membership and launch the daemon control worker with a loopback NAP fake. Wait for the guarded TCP listener and verify that its address is bound.
2. Replace the persisted membership bytes with an invalid document while the fake NAP continues reporting ownership.
3. Assert the worker retires and closes the existing listening socket within a bounded interval, without accepting a new authenticated session.
4. Wait beyond the normal two-second rebind interval. Assert no new bind attempt occurs, and verify the invalid bytes were not silently repaired.
5. Restore valid membership explicitly through the owner-controlled store API. Assert the worker obtains a fresh guarded listener, and shutdown releases its socket.
6. Run formatting, locked workspace checks, Clippy, daemon unit tests, and integration CI at the exact resulting master SHA.

**Limitations:** This test uses loopback TCP and a fake NAP. It does not qualify mutual-TLS enrollment UX, PANU join, radio link behavior, or physical hardware acceptance. Keep P7-003/P7-004/P6-004 in progress.

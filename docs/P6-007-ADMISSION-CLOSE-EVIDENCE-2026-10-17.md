# P6-007 terminal NAP owner admission lifecycle (software only, 2026-10-17)

The in-process `NapAdmissionLedger::close` is an atomic, idempotent and terminal transition. It clears pending and active reservations under the same mutex used by reserve/activate, rejects subsequent admission, activation and capability refresh, and permits harmless stale retire callbacks. `NetworkNapAdmission::close` forwards the transition while retaining the private instance token; a newly constructed NAP owner has a distinct token, even for the same logical network and peer generation.

Deterministic tests cover pending and active invalidation, repeated close, 16 competing admission/activation threads, rejected reopen, and isolation of a replacement NAP owner. These tests do **not** prove that physical PANU sockets disconnect, that a daemon calls close on all teardown paths, or that attached peers are authenticated. Production daemon integration, independently confirmed TLS enrollment, live attachment observation, concurrent PAN traffic and physical adapter-count acceptance remain open. Do not mark P6-007 complete based on this primitive.

Qualification: run exact-head CI on the commit adding this file; record the resulting SHA/run separately after completion.

# P7-004 — Obsolete TLS owner I/O guard (2026-10-16)

**Status: IMPLEMENTED AND SOFTWARE-QUALIFIED; LIVE PAN RUNTIME NOT INTEGRATED.**

The initial predicate prototype was unreferenced. It is now compiled into the daemon through `apps/daemon/src/registered_transport.rs` and `apps/daemon/src/registered_transport/generation_guard.rs`.

`RegisteredControlSession` requires its exact `(NetworkId, NodeId, TLS exporter session ID)` registration to remain in the `Active` phase. Checks run before and after `receive_next_at`, `send_heartbeat_until`, `send_goodbye_until` and `revalidate_approval`. Failed checks close the owned TLS/TCP socket **before** removing only the exact generation; poisoned registry locks also fail closed. Goodbye's post-write check allows the stream's terminal framing state while requiring the registry generation to remain active. This is a best-effort I/O ownership fence; it does not make a concurrent network write atomic with a registry transition.

**Qualification:** Exact-head [CI 38985459933](https://github.com/ekkus93/blueroute/actions/runs/38985459933) passed at `a6a5fdb8ace6ad317accbecb483bd14cafd2ea3f`. A deterministic registry test covers wrong network, wrong session, loss, reconnect and replacement. A localhost mutual-TLS integration test in `apps/daemon/src/approved_transport.rs` checks that an obsolete registered owner cannot deliver frames after a concurrent synthetic generation replacement, and that dropping the old owner preserves the new slot. Exact-head [CI 38985689554](https://github.com/ekkus93/blueroute/actions/runs/38985689554) passed at `35887cd197f6dfca973d8bfd4a4fd874224e2570`.

**Remaining:** The test uses real TLS but a simulated replacement registry entry. It does not verify a real PAN loss callback or production daemon listener/connector. Implement daemon-owned PAN transport lifecycle and reconnect callbacks, independent key-enrollment verification UX, production authenticated JoinNetwork and deferred two-machine Bluetooth acceptance. Do not mark P7-004 or P6-004 complete.

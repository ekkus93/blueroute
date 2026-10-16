# P7-004 — Obsolete TLS owner I/O guard (2026-10-16)

**Status: PARTIAL, NOT INTEGRATED.** A protocol integration regression at `crates/blueroute-protocol/tests/registry_loss_replacement.rs` proves that a stale PAN loss/removal cannot retire a newly authenticated replacement registry generation. Exact-head [CI 38980903590](https://github.com/ekkus93/blueroute/actions/runs/38980903590) passed at `e6ab15c65706399c7f2fa5badef5a4fb2b180479`.

An inactive, unreferenced predicate prototype exists at `apps/daemon/src/registered_transport/generation_guard.rs`. **It is not compiled into the daemon, called by the registered transport, or qualified as an I/O guard.** The source-edit safety boundary prevented updating the larger `registered_transport.rs` and rejected removal of the unused prototype.

Next: require active matching registry generation before/after TLS read/write and on approval recheck; close obsolete sockets before exact-generation retirement. Add real TLS read-race regression. Live daemon/PAN callbacks, independent enrollment, production JoinNetwork and physical Bluetooth acceptance remain open.

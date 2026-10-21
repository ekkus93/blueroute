# P7-005: exhaustive premature-EOF control transcript qualification

**Date:** 2026-10-21 UTC  
**Source:** `a6f04bed980c2764b7296bb1e159a623f6232e97`  
**Exact-head CI:** [39591637061](https://github.com/ekkus93/blueroute/actions/runs/39591637061) — **passed**.

The regression in `crates/blueroute-protocol/tests/control_session_fuzz.rs` exercises every premature EOF offset in an otherwise valid Heartbeat / Heartbeat / Goodbye transcript, under four input-fragment widths. Every truncated transcript must reject EOF and permanently poison the parser. The complete transcript must still accept graceful Goodbye + EOF. This extends P7-005 deterministic parser qualification without changing production control-session behavior.

**Scope limitation:** The test constructs a simulated already-verified session; it does not establish TLS peer identity, independently enroll keys, run a daemon-owned authenticated PAN control loop, implement production P6-004 JoinNetwork, or prove any Bluetooth radio behavior. P7-003/P7-004/P7-005/P6-004 remain open, and hardware acceptance remains deferred.

**Documentation-write limitation:** An attempt to add this evidence directly to the complete 3,174-line authoritative `docs/TODO.md` was denied by platform safety checks before a commit. This small evidence record preserves the qualification without truncating or overwriting the TODO. Reconcile it into the TODO only through an allowed, full-content-preserving write.

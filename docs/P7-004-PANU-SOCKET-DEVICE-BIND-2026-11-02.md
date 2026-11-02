# P7-004 — Linux device-bound outbound PANU TCP control dial

**Date:** 2026-11-02 UTC  
**Scope:** Software-only pre-dial routing isolation; not production JoinNetwork acceptance  
**Implementation:** `54932caad0ace77af14579a67ef4ab37704d06cd`  
**Dependency lock correction:** `7cf5818702a26c362a2fa83f865da625c0a72a8b`  
**Clippy correction / qualified source:** `512f32fa85840371574654f50f6ed0b274b9c51d`  
**Exact-head CI:** [run 41505918201](https://github.com/ekkus93/blueroute/actions/runs/41505918201) — **PASSED**.

## Security change

`apps/daemon/src/outbound_control.rs` now calls `connect_tcp_from_ipv4_on_device` rather than the source-only TCP dial. The Linux helper sets `SO_BINDTODEVICE` on the unconnected socket using the locally observed PANU interface, binds the deterministic BlueRoute-owned source IPv4 address, then attempts the deadline-limited TCP connection. A missing interface or kernel permission denial returns an error; **there is no unbound fallback**. Existing NetworkManager ownership and active-prefix checks, durable peer authorization, pinned mutual TLS 1.3, exporter-bound Hello/HelloAck and session-registry admission remain in force.

This closes the gap in the earlier point-in-time NetworkManager checks: a source-address-bound SYN could otherwise use an unintended interface after a routing change. The kernel now restricts the socket to the selected interface before any SYN. Invalid Linux device names (empty, embedded NUL, or at least 16 bytes) are rejected before socket allocation.

## Tests and limitations

Linux adapter tests reject nonexistent and malformed interface names without allowing a loopback listener to accept a connection. A separate `lo` device-bound loopback test exercises successful connection where runner privileges allow `SO_BINDTODEVICE`; if the kernel denies that option, the test treats denial as safe and does not claim successful binding. The source-only helper retains its existing tests for non-PAN callers.

Exact-head CI passed formatting, `cargo check --workspace --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, Rust tests, simulated sockets, real-broker integration and packaging. Earlier intermediate runs failed due to a `socket2` 0.6.1 lockfile mismatch and a Clippy warning, corrected before qualification. The new dependency is pinned to `socket2` 0.6.3 with consistent `Cargo.lock`.

**Still open:** independent user-verified TLS key enrollment (P7-003); production `LinuxJoinRuntime` PANU wiring, authenticated session ownership and bounded reconnect/loss supervision (P7-004); production `JoinNetwork` (P6-004); two-node Bluetooth acceptance. No radio traffic or privileged production PANU route was exercised. Do not mark these tasks complete.

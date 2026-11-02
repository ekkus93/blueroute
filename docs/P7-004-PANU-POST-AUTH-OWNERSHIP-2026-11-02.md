# P7-004 — Outbound PANU post-authentication ownership gate (2026-11-02)

## Scope

The outbound PANU mutual-TLS 1.3/Hello bootstrap previously checked the
BlueRoute-owned NetworkManager IPv4 lease and exact BlueZ PANU attachment only
**before** TCP connection. A link could detach, change interface, or lose
its owned IPv4 address while TLS and Hello were running. A successful TLS
handshake alone was not evidence that the original PANU was still present.

The bootstrap now repeats fresh NetworkManager owned-address and active-prefix
observations and exact BlueZ peer/interface attachment verification **after**
mutual TLS, exporter-bound Hello, and registry admission, before returning the
registered session. It also rechecks durable (NetworkId, NodeId, SPKI)
authorization after the potentially blocking observations and rejects an
expired handshake budget. Any error drops the registered session owner, whose
existing Drop implementation closes the TLS socket and retires the exact
registry generation. There is no fallback to an unverified session.

The regression in `apps/daemon/src/outbound_control.rs` uses a drop-tracked
owner to prove that injected post-handshake PANU loss and authorization
revocation discard the owner, while successful validation retains it.

## Limitations

This is a **point-in-time post-authentication check**, not continuous PANU
link supervision, a kernel-atomic attachment lease, a production
`LinuxJoinRuntime` connection, or a user-approved first-key enrollment
ceremony. It does not enable production `JoinNetwork`. Bluetooth hardware
acceptance remains deferred. P7-003, P7-004 and P6-004 stay open.

CI evidence should be associated with the exact implementation commit; do
not mark acceptance complete without its exact-head verification.

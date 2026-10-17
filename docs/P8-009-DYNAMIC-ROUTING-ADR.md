# P8-009 — Dynamic routing protocol decision

**Status:** Accepted (design decision only), 2026-10-17.  
**Decision:** Retain BlueRoute-owned, authenticated, deterministic route planning and Linux-backend reconciliation for the initial routed LAN. Do not deploy Babel or another independent routing daemon by default. Revisit after measurable multi-hop qualification.

## Context

`docs/SPEC.md` §§14–19 requires distinct private prefixes per PAN segment, Linux layer-3 forwarding, route ownership, bounded recovery, and authenticated membership. P7 live enrollment/control, P8 route distribution/application, P9 runtime topology and real Bluetooth multi-hop acceptance remain open. Existing deterministic topology tests do **not** measure actual convergence, overhead or packet loss.

[Babel RFC 8966](https://www.rfc-editor.org/info/rfc8966) standardizes a loop-avoiding distance-vector protocol with sequence-based feasibility, link-quality metrics, and IPv4/IPv6 support; it has periodic routing traffic and documented prefix-retraction caveats. [RFC 8967](https://www.rfc-editor.org/info/rfc8967) defines MAC-based routing authentication and replay mitigation; [RFC 8968](https://www.rfc-editor.org/info/rfc8968) defines a DTLS alternative. Neither RFC authenticates BlueRoute membership by itself. No Babel implementation has been exercised on BlueRoute PAN links.

## Alternatives and tradeoffs

| Criterion | BlueRoute orchestration (selected for v1) | Babel (deferred) |
| --- | --- | --- |
| Convergence | Explicit observation/diff/retry; deterministic tests exist, real reconvergence unmeasured. | Standardized distributed loop avoidance and reconvergence; real PAN timing unmeasured. |
| Security | Can bind route authority to existing verified `NetworkId`, `NodeId`, and pinned mutual TLS. | Requires a separate routing-plane key/identity lifecycle and strict interface/neighbor authorization; default unauthenticated acceptance is forbidden. |
| Observability | Desired/observed route diff and typed ownership errors can be surfaced through daemon diagnostics. | Protocol neighbor/route telemetry requires new daemon correlation and operator diagnostics. |
| Complexity | Existing Rust core and Linux network-backend boundaries; BlueRoute must implement distribution and recovery. | Mature algorithm but additional daemon/service, authentication, packaging, config and failure modes. |
| Portability | Linux kernel routes and backend-neutral application; hardware qualification pending. | Routing daemon availability/configuration on each supported Linux system must be verified. |
| Link overhead | Event-driven bounded control is possible, not yet measured. | Periodic updates/probes consume some bandwidth; no comparative PAN measurements exist. |
| Route ownership | Single explicit owner preserves foreign/default routes. | Two competing route writers require routing-table/metric separation and cleanup coordination. |

OSPF/IS-IS are not selected for this small dynamic PAN topology because their adjacency and deployment model adds operational complexity; no unsupported performance comparison is asserted. Static per-node routes remain a test oracle, not autonomous recovery.

## Non-negotiable implementation constraints

1. Accept route/topology information only from an active, durably authorized, mutually authenticated control peer bound to the exact network. Bluetooth trust, MAC/IP addresses, advertisements and unverified Hello are not authorization.
2. Apply and remove only BlueRoute-owned prefixes, interfaces and routes; preserve foreign/default routes and never implicitly enable Internet sharing.
3. Reconcile against freshly observed state after restart, partition, duplicate/stale event, partial failure or revocation. Reject conflicting prefixes and stale session generations.
4. Keep routing policy hardware-agnostic and kernel changes behind the Linux backend. Test partitions, loops, alternate routes, revocation and rollback with deterministic fakes and namespaces/veth before software acceptance.
5. Preserve P7/P8/P9 open status and all physical Bluetooth acceptance gates.

## Revisit criteria and qualification plan

Reconsider Babel after an authenticated A–B–C software path and a repeatable multi-hop physical testbed exist, or sooner if measured convergence or implementation complexity blocks delivery. Compare identical failure/rejoin traces for route convergence, loops/blackholes, TCP/UDP interruption, control bytes, CPU/RAM, revocation, restart, foreign-route preservation and teardown. Record exact builds, Linux/BlueZ/NetworkManager versions, controller/firmware, topology, route tables and logs. Evaluate RFC 8967 versus RFC 8968 key rotation and explicit BlueRoute identity binding without insecure fallback. Issue a replacement ADR only on measured evidence.

**Consequence:** P8-009's comparative design task is complete; this ADR does not implement P8-005/P8-008, prove live routing or qualify Bluetooth behavior.

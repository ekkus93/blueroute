# P7-004 — Idle registered TLS revocation recheck evidence (2026-10-16)

## Scope and implementation

The authenticated `ActiveControlSession` exposes a crate-internal durable approval recheck that sends no frames. `RegisteredControlSession::revalidate_approval()` invokes it, and on a terminal/revoked result shuts down the owned TLS/TCP socket **before** retiring only its exact `(NetworkId, NodeId, exporter session ID)` registry generation. It does not infer authority from BlueZ trust, source IP, MAC address or an unverified Hello.

Changes were committed directly to `master`: `bdd749f58e4bdd3c81971f317497e8d64d11af05` (active-session recheck), `e8365c27c60ea294558158d42bccb7e2dd4215cd` (registry ownership), `5d9c14d64008df675aa5d63a16527cab86cd53cd` (real localhost mutual-TLS regression), and `20dcb3a07519e9fa7cfcb588987ca62af57203ce` (rustfmt repair).

## Qualification

[Exact-head CI run 38968361333](https://github.com/ekkus93/blueroute/actions/runs/38968361333) **passed** for `20dcb3a07519e9fa7cfcb588987ca62af57203ce`. The regression establishes two independently generated test identities with synthetic durable approval, registers the real TLS 1.3 session, revokes the server-side peer approval while idle, calls the recheck without reading any inbound frame, and verifies that the registry slot is removed and the client sees a terminal transport error **before** the server session owner is dropped. Earlier [run 38968043560](https://github.com/ekkus93/blueroute/actions/runs/38968043560) failed formatting only and was corrected.

## Remaining acceptance

**P7-004 stays IN PROGRESS.** The running daemon does not yet own a PAN-bound listener or session supervisor invoking this recheck on approval changes; reconnect-generation scheduling is not connected to live PAN loss callbacks. P7-003 independent user-confirmed enrollment, P6-004 production authenticated JoinNetwork and physical multi-computer Bluetooth qualification remain open. This localhost software regression is not Bluetooth PAN acceptance and must not be used to close those checkboxes.

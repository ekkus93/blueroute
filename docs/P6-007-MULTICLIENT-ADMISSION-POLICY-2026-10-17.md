# P6-007 — Multi-client NAP admission policy (software evidence)

Date: 2026-10-17
Qualified source: `42cede7a93b5bd5fe8e1abbbd06fbc6d48f518dd`
Exact-head CI: [39064441580](https://github.com/ekkus93/blueroute/actions/runs/39064441580) — passed.

`crates/blueroute-core/src/nap_admission.rs` exports a deterministic,
hardware-independent `plan_nap_client_admission` function. It accepts one
node's local `NodeCapabilities`, a caller-supplied set of active client
identities, and an admission candidate. Unknown or unusable NAP/adapter
capabilities and absent client ceilings fail closed. The source of an
explicit ceiling (measured, configured or conservative default) is retained
in the result: no model name or universal Bluetooth controller count is
inferred. Zero ceilings, duplicate/zero identities and already exceeded
capacities are rejected. A new client cannot exceed the ceiling, while an
already-attached peer is idempotently admitted without occupying a second
slot.

Five deterministic tests verify provenance/capacity arithmetic, full-star
idempotence and rejection, unknown/unusable/zero capabilities, invalid
identities and over-capacity observations, and ordering independence.

## Not yet implemented or accepted

This is **software-only policy groundwork**, not a complete P6-007
implementation. The caller must independently authenticate membership and
attachment observations and serialize admissions at the actual daemon/PAN
connection boundary. It does not open Bluetooth connections, implement
concurrent TCP/UDP traffic, verify a hardware controller's simultaneous
connection ceiling, or claim two/three-device physical behavior.

Further acceptance: production P7/P6-004 authenticated PAN integration,
parallel multi-client connections using one NAP, tested traffic between
clients, capacity/diagnostics through the daemon CLI and frontends, and
documented measured per-adapter limits when physical testing resumes.
All three P6-007 acceptance subtasks remain unchecked.

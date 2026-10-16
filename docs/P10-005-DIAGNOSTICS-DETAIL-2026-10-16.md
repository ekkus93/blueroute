# P10-005 — Detailed read-only CLI diagnostics (2026-10-16)

**Status:** SOFTWARE PARTIAL. The production daemon does not yet supply live PAN interface/route/topology observations, and no physical Bluetooth acceptance is claimed.

The `blueroute diagnostics show` human renderer now exposes every already-defined optional diagnostic collection: observed peers, interfaces, routes, topology nodes/links and bounded recent API failures. It distinguishes **unavailable** (the daemon did not collect this observation) from **none observed** (an explicit empty result). Per-adapter powered state, address and driver are displayed rather than only an adapter count.

Diagnostic string fields use Rust Debug escaping to prevent raw terminal control characters supplied through a daemon snapshot from being printed as terminal escape sequences. The `--json` representation is unchanged and retains typed round-trip serialization. The new deterministic regression covers empty/missing/present observations, adapter detail, terminal escaping and JSON fidelity.

**Remaining:** populate real Linux interface/route/topology observations from owner-aware backends, qualify real daemon/broker diagnostic snapshots, review redaction when new fields are added, and complete P7 authentication before treating remote reachability as authoritative.

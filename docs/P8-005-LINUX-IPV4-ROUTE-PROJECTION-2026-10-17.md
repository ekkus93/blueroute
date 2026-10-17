# P8-005 — IPv4 route projection software evidence

Date: 2026-10-17
Qualified source: `bb407ff20cac23a67a7dcdbb70037d578797febd`
Exact-head CI: [39049461622](https://github.com/ekkus93/blueroute/actions/runs/39049461622) — passed.

The public `project_ipv4_route_intents` function in
`crates/blueroute-linux/src/route_projection.rs` translates logical
per-network routes to deterministic `LinuxRoute` descriptions without
performing privileged mutations. Caller-provided destination and first-hop
bindings are checked for network scope, identity, canonical private IPv4
prefixes, duplicate kernel destinations and missing observations. Direct
host destinations use on-link routes; routed hosts use a first-hop gateway.
Four deterministic regression tests cover normal mapping, foreign or absent
bindings, unsafe prefixes and duplicate kernel destinations.

These bindings are **not proof of authentication or kernel ownership**.
The daemon must supply verified current membership/PAN/interface observations.
A privileged backend must independently reobserve and atomically check kernel
ownership at the actual write site. Authenticated distribution, runtime
integration, Linux namespace qualification and real Bluetooth multi-hop
acceptance remain open. P8-005 is still software-partial.

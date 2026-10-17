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
Five deterministic regression tests cover normal mapping, foreign or absent
bindings, unsafe prefixes, duplicate kernel destinations and the node /32
host-prefix requirement.

These bindings are **not proof of authentication or kernel ownership**.
The daemon must supply verified current membership/PAN/interface observations.
A privileged backend must independently reobserve and atomically check kernel
ownership at the actual write site. Authenticated distribution, runtime
integration, Linux namespace qualification and real Bluetooth multi-hop
acceptance remain open. P8-005 is still software-partial.

The node host-prefix guard passed [CI 39049814583](https://github.com/ekkus93/blueroute/actions/runs/39049814583) at `a42f660`.

## Direct host address/identity binding (2026-10-17)

A logical node whose bound IPv4 `/32` equals the first-hop address may not be projected as an on-link direct destination unless its NodeId is the first-hop NodeId itself. Otherwise the projection fails closed rather than silently redirecting an unrelated identity. The negative and matching-neighbor positive regression passed [exact-head CI 39053134616](https://github.com/ekkus93/blueroute/actions/runs/39053134616) at `34d9288193e67440dd3e8cc71bd2eae699bdce5b`. This does not authenticate caller-provided node-to-IP bindings or demonstrate kernel/Bluetooth behavior.

## Unique peer-to-address bindings (2026-10-17)

Untrusted caller-supplied route binding arrays now reject duplicate IPv4 destination prefixes across distinct logical destinations and duplicate next-hop IPv4 addresses across distinct NodeIds, regardless of which routes are currently requested. Two new deterministic regression tests also confirm valid distinct next-hop address inputs remain accepted. [Exact-head CI 39055838456](https://github.com/ekkus93/blueroute/actions/runs/39055838456) passed at `942f8ea85a7301dd68784e497438a77218029fa3`. This is **not** verified peer/address mapping, authenticated distribution, privilege-site route ownership or physical PAN evidence.

## Cross-peer destination/observed next-hop identity aliasing (2026-10-17)

A NodeId destination may not claim an IPv4 /32 address observed for a
different next-hop NodeId, even if the suspicious destination binding is
unused by the current routes. Checks occur before route projection and do
not depend on destination or hop input order. The previously ignored
regression was enabled and positive direct/transit coverage added.
[Exact-head CI 39060062626](https://github.com/ekkus93/blueroute/actions/runs/39060062626)
passed at `f148037eabeb87b99858324de58962bd851de633`.
The privilege-site kernel state and true PAN peer identity are still not verified.

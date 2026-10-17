# P8-005 cross-peer IPv4 binding review

Review date: 2026-10-17. Implemented at `f148037eabeb87b99858324de58962bd851de633`; [exact-head CI 39060062626](https://github.com/ekkus93/blueroute/actions/runs/39060062626) passed. This is software-only regression evidence, not authenticated identity or kernel/Bluetooth acceptance.

The IPv4 route projector checks whether a destination node claiming the selected next-hop IPv4 address has the same NodeId. It does not reject a destination NodeId claiming the IPv4 address of a *different* observed next-hop identity when the route uses another next hop. This can produce ambiguous peer/address bindings from untrusted inputs.

Before privileged route application, reject every NodeId destination binding that aliases an observed next-hop IPv4 address owned by another NodeId, even when that destination binding is not selected by a route. Preserve a positive test for a real directly reached peer and tests for transit through a distinct peer. These checks are not a substitute for authenticated PAN binding or kernel ownership verification.

## Regression qualification

The route projection now compares every destination NodeId's IPv4 /32
binding with **all** observed next-hop IPv4-to-NodeId bindings, including
unused bindings. A destination claiming another peer's next-hop address
fails before producing any route intents. It remains valid to route through
a different next-hop peer when destination and next-hop identities are distinct.

The previously ignored test now runs, accompanied by two integration cases
for input-order independence and legitimate direct/transit paths. Formatting,
locked workspace checks, strict Clippy and the full CI workflow passed at
`f148037eabeb87b99858324de58962bd851de633` (run 39060062626).
The required authenticated observation source and ownership-safe privileged
kernel route writer are not implemented or qualified by these tests.

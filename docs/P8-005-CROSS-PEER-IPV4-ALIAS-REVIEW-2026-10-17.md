# P8-005 cross-peer IPv4 binding review

Review date: 2026-10-17. This is a proposed hardening change, not implementation or acceptance evidence.

The IPv4 route projector checks whether a destination node claiming the selected next-hop IPv4 address has the same NodeId. It does not reject a destination NodeId claiming the IPv4 address of a *different* observed next-hop identity when the route uses another next hop. This can produce ambiguous peer/address bindings from untrusted inputs.

Before privileged route application, reject every NodeId destination binding that aliases an observed next-hop IPv4 address owned by another NodeId, even when that destination binding is not selected by a route. Preserve a positive test for a real directly reached peer and tests for transit through a distinct peer. These checks are not a substitute for authenticated PAN binding or kernel ownership verification.

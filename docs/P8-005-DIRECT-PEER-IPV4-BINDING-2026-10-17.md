# P8-005 — Direct-peer IPv4 binding consistency

**Date:** 2026-10-17  
**Scope:** software-only Linux route projection hardening.

The projector now rejects a logical route whose destination is the observed
first-hop NodeId but whose claimed IPv4 /32 destination differs from that
peer's observed next-hop IPv4 address. Previously this became a gateway route
*via that same NodeId*, silently treating an unverified alternate address as
belonging to the peer. A deterministic negative regression and a valid direct
peer positive regression cover the distinction.

This is a conservative fail-closed policy for the existing one-address
RouteNextHopBinding model. A future authenticated multi-address design must
carry separate evidence and explicitly revise this restriction; it must not
infer extra peer addresses from untrusted route destinations.

**Qualification:** exact-head CI [39062070393](https://github.com/ekkus93/blueroute/actions/runs/39062070393) passed at `6398702fc339c4739fc488f7df3ceebdfbcdc0ed` after the formatting repair (initial implementation `85eb555269c43fb39b664068eb1fe9af8b6032ec`).
**Still open:** independently authenticated PAN/IP observations, privileged
kernel route mutation, daemon lifecycle integration, production JoinNetwork,
and real Bluetooth/multi-hop acceptance. This does not prove physical behavior.

## Unused destination versus observed peer address (2026-10-17)

A direct peer represented by an observed `RouteNextHopBinding` must also
have its NodeId destination /32 bound to the **same** IPv4 address, even if
that destination is not needed by the selected routes. Previously, an
unused `RouteDestinationBinding` could quietly claim an unverified second
address for a directly observed peer, contradicting the one-address model.

The projector now rejects this mismatch before returning any route intent.
The new integration test covers both destination orders and both observed
next-hop orders, while retaining a positive distinct transit-destination
case. The software change and formatting repair passed [exact-head CI
39063939963](https://github.com/ekkus93/blueroute/actions/runs/39063939963)
at `b618fefd78f5b01c381b723ff027c236ae282faf`.

This check does not establish peer authentication or verified IP ownership:
the next-hop observations remain caller-supplied. No privileged route
mutation, authenticated distribution, production JoinNetwork, or physical
Bluetooth acceptance is claimed. P8-005 stays **SOFTWARE PARTIAL**.

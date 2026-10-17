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

**Qualification:** pending exact-head CI for the implementation commit.
**Still open:** independently authenticated PAN/IP observations, privileged
kernel route mutation, daemon lifecycle integration, production JoinNetwork,
and real Bluetooth/multi-hop acceptance. This does not prove physical behavior.

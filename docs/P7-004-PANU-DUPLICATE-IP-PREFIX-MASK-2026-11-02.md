# P7-004 — Reject duplicate PANU source IPv4 addresses with different prefix masks

**Date:** 2026-11-02 (UTC)  
**Scope:** Software-only outbound PANU control pre-dial guard  
**Implementation:** `ca13f62fcc4432b51387fabdfd4f21167149e590`  
**Formatting/test corrections:** `8b7a4fbd3303cf0d3105f83e2a942e0ab58275fa`, `f2439b43a0519f3be869188dd5969d29c718d01d`  
**Exact-head CI:** [run 41485138659](https://github.com/ekkus93/blueroute/actions/runs/41485138659) — **PASSED** at `f2439b43a0519f3be869188dd5969d29c718d01d`.

## Problem and correction

The outbound control preflight in `apps/daemon/src/outbound_control.rs` already rejects duplicate NetworkManager-owned and active PANU source prefixes. However, its ambiguity checks compared the **whole** `IpPrefix`, including the mask. A second interface could claim the **same IPv4 host address** with a different prefix length (for example, `10.x.y.2/24` and `10.x.y.2/32`) and evade the duplicate-source guard. The TCP source bind alone cannot distinguish the two owners.

The guard now checks `IpPrefix.address` for **conflicting owned addresses** and **active addresses on another or unknown interface**, regardless of mask. The expected BlueRoute-owned lease and the exact active interface/prefix must still be present, and the existing TLS pin, durable approval, source bind, exporter-bound Hello and session registry checks remain mandatory.

## Deterministic regression

`outbound_control::tests::ambiguous_duplicate_panu_source_fails_closed_even_when_expected_lease_is_present` now adds two cases in which a foreign interface reports the same source IPv4 address with a `/32` mask while the expected PANU lease uses `/24`. One case injects duplicate NetworkManager ownership; the other injects a duplicate active-interface observation. Both must fail with `InvalidState`, while the original unambiguous owned `/24` fixture continues to pass.

The initial CI attempt failed rustfmt; the second attempt exposed an incorrectly chosen identical `/24` test mask. Both were corrected before the successful exact-head run.

## Limits and remaining acceptance

This is a point-in-time software-only pre-dial check. It does **not** prove that the Linux routing table will keep using the PANU interface after the observation, provide socket-level interface binding, perform independently authorized peer-key enrollment, wire the outbound session into production `LinuxJoinRuntime`, implement reconnect supervision, enable production authenticated `JoinNetwork`, or establish physical Bluetooth interoperability.

Keep **P7-003, P7-004, P7-005 and P6-004 IN PROGRESS**; leave hardware acceptance deferred. The next priority is production outbound PANU integration and operator-verified key enrollment, not more isolated parser cases.

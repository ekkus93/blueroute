# P7-004 — Ambiguous PANU source ownership denial (software evidence)

**Date:** 2026-11-02 (UTC)  
**Status:** SOFTWARE PARTIAL; P7-003/P7-004/P6-004 remain IN PROGRESS; hardware acceptance DEFERRED.

## Change

The outbound PANU control pre-dial guard in `apps/daemon/src/outbound_control.rs` previously accepted any current BlueRoute-owned IPv4 lease and matching active interface. It now additionally rejects a snapshot if the exact IPv4 source address/prefix appears under a different NetworkManager owner/interface or on an additional active interface, including unknown interface provenance.

This matters because an expected lease appearing in a snapshot does not prove that the address is unique. A source-address-bound TCP socket alone cannot guarantee routing through the intended PANU when duplicate source addresses exist.

## Deterministic regression

`ambiguous_duplicate_panu_source_fails_closed_even_when_expected_lease_is_present` covers:

- Valid expected lease and active interface: accepted.
- Same source address additionally owned on another interface: rejected.
- Same source address additionally owned by a different logical network: rejected.
- Same source address additionally active on another interface: rejected.
- Same source address additionally active with unknown interface provenance: rejected.

These are synthetic backend observation fixtures; they do not send a real Bluetooth packet or establish continuous routing ownership.

## Verification

- Implementation: `181bba25ebd14cd059e9c8b4c2d25db23b23d5e4`.
- Rust formatting correction: `1b56102af3166d7797c5cfe04fe300f76e569fea`.
- Exact-head CI: [run 41476679638](https://github.com/ekkus93/blueroute/actions/runs/41476679638), **PASSED** (format, locked workspace build, Clippy, Rust tests and integration checks).

## Remaining acceptance

This is a point-in-time software guard. It does not implement socket-level device binding, continuous PANU ownership monitoring, independent operator-approved key enrollment, outbound PANU session lifecycle/reconnect, production authenticated `JoinNetwork`, or physical Bluetooth acceptance. Do not mark P7-003, P7-004 or P6-004 complete.

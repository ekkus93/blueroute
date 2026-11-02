# P7-004 — Fresh BlueZ PANU attachment pre-dial guard

Date: 2026-11-02 (UTC)

## Problem

The outbound authenticated PANU control helper already checks the durable
peer SPKI pin, the deterministic NAP endpoint, NetworkManager's current
BlueRoute-owned IPv4 lease, active-prefix/interface provenance, and Linux
socket device binding. A stale NetworkManager address, however, could
remain configured after the BlueZ PANU link was disconnected or replaced.
Address ownership alone is not proof of an active Bluetooth PANU link.

## Change

- Added `BluezBackend::require_matching_panu` to re-observe the current
  BlueZ Network1 attachment for the exact expected peer and interface.
- A disconnected PANU now rejects the dial; a changed interface or peer
  also rejects the dial without attempting to reuse the old join handle.
- Exposed the original `JoinPanLink::attachment()` read-only so the
  outbound helper cannot reconstruct a weaker identity from an interface
  name alone.
- Called the BlueZ check **after** the NetworkManager ownership snapshot
  and **before** the source/device-bound TCP dial; the existing durable
  peer-key authorization recheck still runs after those potentially
  blocking backend observations.
- Added deterministic regression cases for matching, absent, replaced,
  wrong-peer, wrong-role and peerless PANU attachments.

## Verification

- Implementation: `d9659e8a0312be7b163d206d8cd3b5a2feb5d408`.
- Formatting correction: `c2806b2fece0631eb2d4ab0056e943da7a6522e6`.
- Exact-head CI: [run 41523865608](https://github.com/ekkus93/blueroute/actions/runs/41523865608), **passed** formatting, locked workspace compilation, Clippy, Rust tests and integrations.
- Initial implementation CI failed rustfmt only; the correction passed.

## Limitations and next steps

This is a point-in-time BlueZ observation, not an atomic lease across BlueZ
state changes and the subsequent TCP SYN. Socket `SO_BINDTODEVICE`
continues to enforce the selected interface, but continuous PANU loss
observation, reconnect ownership, independently approved key enrollment,
and production `LinuxJoinRuntime` join orchestration are still missing.
The test uses synthetic PANU observations; it does not establish live
Bluetooth behavior. Keep P7-003/P7-004/P6-004 open and physical acceptance
deferred.

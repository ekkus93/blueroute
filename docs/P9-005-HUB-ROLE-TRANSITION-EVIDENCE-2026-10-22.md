# P9-005 — Observed NAP-to-PANU role transition

**Date:** 2026-10-22 UTC  
**Status:** Software-only regression qualified; runtime and physical acceptance remain open.  
**Implementation:** `9f913f913596d297e26a6af78efc46071c26b86d`  
**Exact-head CI:** [39856064463](https://github.com/ekkus93/blueroute/actions/runs/39856064463) — passed.

## Problem

The hub-loss tracker previously treated a formerly observed NAP as a failed
node after its last hosted link disappeared, even when the same NodeId was
independently observed as a PANU client on another hub. A valid role change
could therefore cause a false loss notification, and the recovery planner
would reject the still-observed node as a supposedly unavailable hub.

## Software change

`HubLossTracker::observe` now distinguishes absent hub role from absent
node. A former NAP that is observed as a PANU client is retired from
missing/lost-hub tracking. If that node later hosts a NAP again, its positive
observation re-arms loss detection. A previously confirmed loss is also
cleared when the node is independently observed as a PANU client.

The deterministic `observed_hub_to_client_role_change_does_not_invent_node_loss`
regression checks demotion, elapsed confirmation intervals, re-arming,
genuine subsequent loss, and recovery of a previously confirmed missing hub
as a client.

## Boundaries

This is a pure software policy over independently observed PAN links.
It does **not** authenticate discovered peers, implement daemon-owned BlueZ
role changes, reconcile live routes, enable production JoinNetwork, or prove
Bluetooth radio recovery. P9-005/P9-006 remain open; hardware acceptance
remains deferred under the project's software-first policy.

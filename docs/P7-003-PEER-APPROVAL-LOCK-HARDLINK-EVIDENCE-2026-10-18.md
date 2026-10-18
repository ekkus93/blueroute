# P7-003 peer-approval lock hardlink rejection (software-only)

**Date:** 2026-10-18 UTC  
**Implementation commit:** `3e69c5428f15c87218726f63c88e6c8eca7957b0`  
**Exact-head CI:** https://github.com/ekkus93/blueroute/actions/runs/39110723873 — **passed**.

## Regression

The new `hardlinked_approval_lock_is_rejected_and_recovery_requires_single_link`
test in `apps/daemon/src/peer_pin_store.rs` creates a second hardlink to the
persistent peer-approval lock inode. `PeerPinStore::try_lock` must reject
the resulting multi-link file with `PersistenceError`; after removing only
the unexpected alias, normal locking must succeed. This covers the existing
single-link inode validation and protects against silently accepting an
unexpected alias of the advisory lock.

## Limits

This is deterministic Linux filesystem and process-lock regression coverage,
not proof of protection against noncooperating same-UID writers. It does not
implement independently confirmed key enrollment, mutual-TLS daemon/PAN
integration, production `JoinNetwork`, or physical Bluetooth acceptance.
P7-003/P7-004/P6-004 remain open; the hardware deferral is unchanged.

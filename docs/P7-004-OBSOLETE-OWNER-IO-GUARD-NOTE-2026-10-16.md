# P7-004: obsolete TLS owner I/O guard

Status: design note, not implemented or qualified.

The registry can mark a channel Lost/Reconnecting or replace its generation while the old TLS owner remains alive. Before sending a heartbeat/Goodbye or returning a received frame, the registered owner should require that its `(NetworkId, NodeId, exporter session ID)` still matches an `Active` registry entry. A blocked read must recheck after completion. Missing/poisoned entries must close the owned socket before exact-generation retirement. Stale callbacks must never close a replacement.

Acceptance requires real localhost mutual-TLS regressions for a replaced session and a loss during blocking read, plus exact-head CI. This does not replace live daemon PAN wiring or physical Bluetooth acceptance.

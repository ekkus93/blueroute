# P7-003 — Local TLS public-key inspection

**Date:** 2026-10-16  
**Scope:** software-only, read-only local credential display  
**Commit:** `4b05c3c4c7e8f35429a1e673a3f401030da17b97`  
**Exact-head CI:** [38929324205](https://github.com/ekkus93/blueroute/actions/runs/38929324205) (passed)

## Operator command

```bash
blueroute peer local-key
blueroute peer local-key --json
```

Shows both the daemon's persistent **local** NodeId and its TLS certificate public-key SPKI SHA-256 fingerprint. The values are obtained using version-checked D-Bus status and diagnostics queries; the CLI does not read private keys, durable enrollment files or raw Bluetooth identities. It rejects absent or zero NodeIds and missing, noncanonical, zero or malformed fingerprints instead of displaying misleading data.

An operator can independently compare these **public** values to an enrollment candidate on another machine. This command does not verify the remote machine, confirm a candidate, write trust/pins, open a socket, or grant membership. The daemon's `GetDiagnostics` fingerprint alone must never be interpreted as a peer's TLS proof of possession; only the authenticated P7 handshake can prove the presented key.

## Remaining gates

- Implement the explicit independent confirmation/first-key enrollment flow with a user-facing decision and a PolicyKit authorization boundary.
- Verify possession of the independently confirmed key using mutual TLS 1.3 and bind Hello to the resulting exporter session.
- Wire the authenticated session to the daemon-owned PAN listener and per-peer reconnect/loss ownership, then integrate production P6-004 with reverse-order rollback.
- Perform physical Bluetooth acceptance later under `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`; neither the CLI tests nor localhost TLS establish RF/PAN behavior.

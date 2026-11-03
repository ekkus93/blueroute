# P7-004 — Bounded authenticated outbound PANU Heartbeat writes (2026-11-02)

## Scope

The exported `run_registered_panu_supervision_until_stopped` worker now emits a TLS-authenticated Heartbeat at most once per five seconds **after** revalidating the original PANU attachment, NetworkManager source-address ownership, and durable pinned peer approval. Each write has a 100 ms absolute deadline, enforced by `RegisteredControlSession::send_heartbeat_until`. A failed write returns an error, closes the owned TLS socket, and retires only that session's registry generation. A stop requested while backend observation was in flight prevents the Heartbeat write.

The helper `send_supervised_heartbeat_if_due` fails closed on regressed clocks and deadline overflow, does not renew the send schedule on error, and has deterministic rate-limit, failed-send, and regressed-clock tests.

## Verification

Source: `apps/daemon/src/outbound_control.rs`. This change must pass exact-head CI before the TODO is reconciled. No physical Bluetooth evidence is claimed.

## Outstanding

This exported worker is **not started automatically by the production daemon** and still does not consume inbound authenticated frames, react directly to BlueZ link-loss notifications, own a production join session, implement bounded reconnect, or perform independently verified key enrollment. P7-003, P7-004, P7-005, and P6-004 remain open; physical acceptance remains deferred.

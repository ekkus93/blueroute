# P7-004 — Control-session lifecycle

Implemented in `crates/blueroute-protocol/src/session.rs` as a pure state machine
(`SessionManager`). It takes injected monotonic time and returns `Action`s
(`Connect`, `SendKeepalive`, `Close`) for the daemon to execute.

- Start: `data_plane_available(peer)` creates the record; nothing connects before it.
- Dialer: only the lower `NodeId` dials, so peers rarely connect to each other at once.
- Loss: silence for `idle_timeout` (default 30 s) closes the session; keepalives go out every
  10 s of send-idle. `heard_from` for a stale session id is ignored.
- Reconnect: exponential backoff 1 s, 2 s, 4 s … capped at 30 s; reset on establishment.
- Duplicates: when two sessions exist for one peer, both ends keep the one initiated by the
  lower `NodeId`, regardless of arrival order; the other gets a `Close` action.
- Leaks: `data_plane_lost` removes the peer record and closes its session; a 200-round
  reconnect test checks no session stays open and no record lingers.

Unproven: execution against a real PAN and transport (needs the handshake task and hardware).

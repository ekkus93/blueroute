# P7-004 stalled-handshake cancellation watchdog

Software-only evidence, 2026-10-15.

The reconnect scheduler requests cancellation once after the aggregate 20-second connect/TLS/Hello attempt budget. It does not schedule a parallel retry. A transport owner must cancel and close the socket before acknowledging failure and scheduling another attempt. Deterministic tests cover stale generations, independent peers, clock regression, deadline boundaries and retry acknowledgment.

Exact-head CI: 38733222967 at 74857c42fb966093474dea5878176cc6166e72c0 (passed).

This is not actual TLS 1.3, key enrollment, authenticated daemon transport, production JoinNetwork or physical Bluetooth acceptance. All those gates remain open.

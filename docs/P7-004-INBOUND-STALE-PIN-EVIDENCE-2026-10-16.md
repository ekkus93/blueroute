# P7-004 inbound stale-pin isolation — 2026-10-16

Software-only evidence for `apps/daemon/src/approved_transport.rs`.

A previously revoked peer's entry in a caller-supplied inbound pin snapshot no longer blocks a separate currently approved peer. The TLS verifier receives only currently approved pins. If no approved pins remain, the owned socket closes before TLS. Invalid network state, process-lock contention and persistence failures still reject the entire attempt. The authenticated active session continues to recheck durable approval.

The localhost mutual-TLS regression covers one revoked and one approved peer; the negative regression checks pre-TLS EOF when all pins are unapproved.

- Qualified commit: `259702cf7cbae779e8fc871f14d2ee15d19c9c31`.
- Exact-head CI: https://github.com/ekkus93/blueroute/actions/runs/38893453716 (passed).
- Earlier formatting-only failure: https://github.com/ekkus93/blueroute/actions/runs/38893301304 (fixed).

This is not a running daemon listener, user-approved key enrollment, production JoinNetwork, or real Bluetooth PAN evidence. P7-003/P7-004 and P6-004 remain open; physical acceptance remains deferred.

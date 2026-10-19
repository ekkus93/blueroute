# P7-004 post-handshake NAP ownership release gate (software only)

**UTC:** 2026-10-19.
**Repository:** `ekkus93/blueroute`, `master`.
**Implementation:** `11adbedc3f723ee3d2768d1ea628cea58e30a08a`.
**Formatting correction:** `024f78c4b87fc00a2130b466887b90ae6c4fc931`.
**Exact-head CI:** [39229133376](https://github.com/ekkus93/blueroute/actions/runs/39229133376) — passed.
**Intermediate CI:** [39228923367](https://github.com/ekkus93/blueroute/actions/runs/39228923367) — failed rustfmt only, corrected in the later commit.

## Behavior and regression

`ObservedControlListener::release_registered_if_current` takes ownership of an
already mutually authenticated, Hello-verified and registry-admitted TLS
session. Before releasing that session to a daemon caller it checks the
session's logical NetworkId and invokes an independent, fresh local NAP
network/endpoint observation. Missing, changed or failed ownership retires
the listener and drops the session owner, closing its TLS socket and
removing only its exact registered generation. A retired listener refuses
release without querying the observation callback.

A localhost regression establishes real pinned mutual-TLS, durable approval,
Hello/HelloAck and registry admission, then checks both matching and
missing post-handshake NAP ownership. The matching case keeps the
registration until explicit cancellation; the missing case rejects release,
permanently retires the listener and leaves the registry empty.

## Acceptance boundary

This is a composed library release gate, **not a running daemon supervisor**.
The daemon does not yet start or poll the NAP listener, independently enroll
a remote key through a user-approved ceremony, attach the accepted session
to a live PANU, or continuously close active sessions on PAN teardown.
The successful observation is not a durable lease. Production P6-004
JoinNetwork remains fail-closed; P7-003/P7-004 remain IN PROGRESS.
No physical Bluetooth or two-device traffic acceptance is claimed.
The authoritative `docs/TODO.md` remains intact at 2,723 lines; this
standalone evidence record avoids retransmitting the entire large TODO.

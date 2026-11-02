# P7-004 — Bounded outbound PANU supervision loop (2026-11-02)

## Scope

The daemon library now exports `run_registered_panu_supervision_until_stopped`, a synchronous worker-thread entry point for a previously authenticated and registered outbound PANU TLS session. It invokes the existing `supervise_registered_panu_control_session` once per second, checking exact PANU attachment, IPv4 lease and durable pinned TLS authorization on each tick.

The loop checks an atomic shutdown flag before and after each observation and during bounded idle waits. Every exit path, including a stop requested before the first tick, invokes `RegisteredControlSession::cancel` to close the owned TLS socket and retire only its exact registry generation. An ownership or authorization failure propagates its typed error after cancellation. The internal scheduler uses bounded 100 ms parks so spurious wakeups cannot suppress subsequent observations.

Deterministic tests cover pre-start cancellation, stop during a tick, repeated successful observations and a third-tick PAN failure. They do not require BlueZ, NetworkManager or a Bluetooth controller.

## Commits and exact-head CI

- Implementation: `d08e19ec27dd80d348d907f8341b5512c57d0706`.
- Formatting correction: `ef9061de7fd55e47c237d6bd9ccd24ee2a31d198`.
- Restore truncated test-module tail: `3660703fcc434634991fbc5af9131dbd95571f9f`.
- Export daemon library API: `e375580e4440eb278dacf214a3f62280a995c803`.
- [Exact-head CI run 41547847820](https://github.com/ekkus93/blueroute/actions/runs/41547847820): **passed** Rust formatting, locked build, Clippy, unit and integration checks.

## Outstanding acceptance

This is a callable, blocking supervision loop, **not an automatically spawned daemon-owned outbound worker**. It does not read authenticated control frames, send heartbeats, subscribe to exact-attachment loss events, or implement bounded reconnect. A production worker must combine those responsibilities and maintain a durable joiner runtime owner before `LinuxJoinRuntime` can enable authenticated `JoinNetwork`. Independent human-confirmed peer-key enrollment and physical Bluetooth acceptance remain outstanding.

Keep P7-003, P7-004, P7-005 and P6-004 **IN PROGRESS**; do not infer physical PAN acceptance from these software tests.

# P7-004 regressed-clock socket cleanup: qualification and CI diagnostics (2026-10-15)

## Scope and security boundary

The daemon's **unwired** `PendingHandshakeSockets` primitive owns pending TCP sockets by logical peer and loss generation. A failure callback closes its socket before updating retry bookkeeping. A regressed monotonic timestamp could previously make `ReconnectScheduler::attempt_failed` return `ClockRegression` *after* the socket was closed, leaving a socketless in-flight retry slot. The correction retires that exact generation on bookkeeping failure. It does **not** schedule a retry on the error, authenticate a peer, run TLS, enroll keys, or enable JoinNetwork.

## Repro, repair and exact-head evidence

- Before: `5e05c9bb9b472fe646a23609bfef53a549840132`, [CI 38751416428](https://github.com/ekkus93/blueroute/actions/runs/38751416428) passed. The old failure path closed the socket and then propagated a scheduler clock error without retiring its retry slot.
- Initial correction: `b48331cc801f428c3ac0073cb00322a5f49eca18`, CAS parent `5e05c9bb9b472fe646a23609bfef53a549840132`, Ralph operation `25fc470921de49168b16aee3141062dc`. [CI 38758225495](https://github.com/ekkus93/blueroute/actions/runs/38758225495) failed **formatting**, not code execution: Rust job `116502251363`, step 6, log lines 154–175 required multiline `assert_eq!` and removal of a trailing blank line. CI created 2026-10-15T15:12:11Z; completed 2026-10-15T15:12:45Z.
- Formatting repair: `98d2c8f743bbad26bf99c253270868ac49330037`, CAS parent `b48331cc801f428c3ac0073cb00322a5f49eca18`, Ralph operation `b04fb79997154de2a06b319dbe83c6a6`. [Exact-head CI 38758360123](https://github.com/ekkus93/blueroute/actions/runs/38758360123) **passed**, created 2026-10-15T15:13:06Z, completed 2026-10-15T15:14:23Z. Formatting, workspace check, Clippy and tests completed successfully.
- Deterministic regression: schedule loss, begin a real localhost TCP socket, report a failure at a timestamp earlier than the last scheduler observation, assert `ClockRegression`, remote EOF and zero owned sockets; assert the expired watchdog cannot recover the retired generation; explicitly schedule a fresh generation and prove stale failure/cancel callbacks do not close it.

## Bridge and failure classification

Ralph Bridge `0.6.2`, build `0acea4b23362e2ccecab8f6c9db4418b162426cf`, instance `51c945dd809b4207800c3ba33151ad94`, started 2026-10-15T02:25:51.132200Z, GitHub App authentication. Both exact-head writes were accepted and path receipts verified. The first failure was an ordinary repository formatting failure, **not** a bridge/API/authorization failure. The subsequent bounded CI wait timed out while tests were still running; a later exact-SHA poll confirmed success. No credentials or private key material were collected.

## Remaining gates

The pending-socket owner is not wired into the daemon's production control listener/dialer. Mutual TLS 1.3 certificate proof, independently confirmed SPKI enrollment, verified TLS exporter, authenticated Hello/HelloAck, revocation of live sessions, and P6-004 production JoinNetwork remain incomplete. Physical multi-computer Bluetooth acceptance is deliberately deferred, not passed. Do not claim v1 release.

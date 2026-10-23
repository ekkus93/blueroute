# P7-004 — Corrupt durable membership gates the running daemon control worker

**Software-only qualification:** 2026-10-22 PDT / 2026-10-23 UTC.

- Test: `apps/daemon/tests/control_worker_corruption.rs`.
- Implementation commit: `32dadf13fcedfc148ee951b3f802bdfd6877f5dc`.
- Rustfmt follow-up: `e2de35dd754745359db0a9cd4841db933ad221f5`.
- Exact-head [CI 39910675086](https://github.com/ekkus93/blueroute/actions/runs/39910675086): **PASSED**, including formatting, locked Rust workspace checks, Clippy and tests.
- An earlier exact-head [CI 39910534642](https://github.com/ekkus93/blueroute/actions/runs/39910534642) failed formatting only; the subsequent commit corrected the reported rustfmt difference.

## Regression coverage

The test launches the actual `DaemonControlWorker` with a fake NAP backend that continues reporting a previously bound endpoint. It verifies all of the following without granting a peer identity based on a TCP source address:

1. Malformed persisted membership blocks initial TCP listener binding despite the fake backend's apparent availability.
2. Explicitly repairing the durable host membership allows a fresh guarded listener.
3. Corrupting the persisted membership while that listener is live retires its real localhost listening socket.
4. The worker refuses to rebind during a full subsequent retry interval while the backend still reports stale ownership.
5. The worker does not silently overwrite, discard or repair corrupt persisted state.
6. A second explicit repair permits a newly bound listener; orderly shutdown releases that socket.

The test uses localhost TCP and an in-process fake backend. It does **not** establish mutual-TLS peer enrollment, an independently verified key ceremony, production PANU/JoinNetwork, daemon process restart acceptance, or physical Bluetooth behavior.

**Status:** P7-003, P7-004 and P6-004 remain **IN PROGRESS**. Required physical acceptance stays **HARDWARE DEFERRED**. Continue with authenticated running-daemon multi-peer/revocation/reconnect fault injection and the independent enrollment ceremony before production JoinNetwork.

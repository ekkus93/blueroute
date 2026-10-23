# P7-004 corrupt durable host integration evidence

Date: 2026-10-22 PDT.
Source test: `apps/daemon/tests/control_worker_durable_corruption.rs`.
Source commit: `6a33d40d09a6ee79c48a977990fb136280720e57`.
Rustfmt-qualified commit: `4e56e5edc52694b091996dc5eb5775d057f37b58`.
Exact-head CI run: 39905110475 — passed.

The integration test launches the actual daemon control worker with a loopback TCP listener and a fake NAP that continues reporting ownership. Malformed durable membership closes the listener; no new bind occurs across the retry interval and the corrupt bytes are preserved. Explicit owner-controlled repair permits a fresh listener; shutdown closes it.

This is software-only evidence. It does not establish independent TLS key enrollment, production JoinNetwork, physical Bluetooth behavior, or complete P7-003/P7-004/P6-004 acceptance.

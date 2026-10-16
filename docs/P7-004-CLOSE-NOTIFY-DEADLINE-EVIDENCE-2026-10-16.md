# P7-004 — TLS Goodbye close-notify deadline evidence (2026-10-16)

## Problem and correction

`ActiveControlSession::send_goodbye_until(deadline)` previously enforced the deadline for the Goodbye frame but called `ControlTlsStream::send_close_notify()` afterward with no absolute deadline. The frame writer restores the original socket timeout before returning, so a blocked TLS close-notify flush could exceed the caller's shutdown budget.

`ControlTlsStream::send_close_notify_until(deadline)` now reuses the **same** absolute deadline, rejects an already expired budget before sending close-notify, temporarily bounds the socket write timeout, checks completion against the deadline, and restores the original timeout on success. A failure is terminal: `ActiveControlSession` closes the owned TLS/TCP socket; `RegisteredControlSession` then retires its exact registry generation. The successful Goodbye path still waits for the peer's clean EOF as required by the existing protocol.

## Software qualification

- Implementation: `cfc17feb9b31cff3f3e822323769fa0ddd2e70ce`.
- Rustfmt correction: `f9e75e4302cd3e3df0b0c9f977086ceb36e1ae9d`.
- [Exact-head CI 38974842950](https://github.com/ekkus93/blueroute/actions/runs/38974842950): **passed**.
- New deterministic regression checks that an already-expired close-notify budget is rejected without a write and without modifying the prior socket timeout. Existing real-localhost mutual-TLS Goodbye and clean-EOF regressions remain green.
- This does **not** prove a bounded close-notify flush under every possible kernel scheduler delay; the post-flush deadline check ensures late completion is rejected, and the socket timeout bounds the blocking write.

## Outstanding acceptance

P7-004 remains **IN PROGRESS**. Actual daemon PAN-bound listener/session supervision, live reconnect callback integration, independently approved P7-003 key enrollment, production P6-004 JoinNetwork and multi-computer physical Bluetooth acceptance are still outstanding. Do not close those requirements based on this software-only result.

# Architecture decision records

This index lists accepted architecture decisions and their remaining implementation gates. Design approval does not imply production implementation or hardware acceptance.

| ID | Decision | Record | Status |
| --- | --- | --- | --- |
| P7-001 | Use TCP over PAN with mutually authenticated TLS 1.3 and explicit per-node SPKI pinning for inter-node control. | [P7-001-CONTROL-TRANSPORT.md](P7-001-CONTROL-TRANSPORT.md) | Accepted; user-verified enrollment UX and live daemon transport integration remain open. |

Other P4/P5/P6 documents are implementation designs, backend contracts, or acceptance procedures rather than separate ADRs. Add future decisions here when a task records an explicit adopt/reject choice.

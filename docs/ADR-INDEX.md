# Architecture decision records

This index lists accepted architecture decisions and their remaining implementation gates. Design approval does not imply production implementation or hardware acceptance.

| ID | Decision | Record | Status |
| --- | --- | --- | --- |
| P7-001 | Use TCP over PAN with mutually authenticated TLS 1.3 and explicit per-node SPKI pinning for inter-node control. | [P7-001-CONTROL-TRANSPORT.md](P7-001-CONTROL-TRANSPORT.md) | Accepted. Implemented in software: the host's inbound pinned listener, the joiner's supervised pinned session (behind the closed join gate), and typed key enrollment on both sides (`EnrollPeerKey`, `RecordJoinHost`). Not physically qualified: no join has run on hardware. |
| P8-009 | Keep BlueRoute-owned, authenticated, deterministic route planning; do not deploy Babel or another routing daemon by default. | [P8-009-DYNAMIC-ROUTING-ADR.md](P8-009-DYNAMIC-ROUTING-ADR.md) | Accepted (design decision only, 2026-10-17). No live routing exists. |

Status words: an **accepted ADR** is an explicit adopt/reject decision; **implemented** means implemented and tested in software; **qualified** means hardware or installed-system evidence passed. Neither implies the next.

## Accepted design records

These record maintainer-accepted designs for a single task rather than cross-cutting architecture choices.

| ID | Design | Record | Status |
| --- | --- | --- | --- |
| P6-004 | Production join wiring (one confirmed host, typed fingerprint, commit-time pinning). | [P6-004-JOIN-ORCHESTRATION.md](P6-004-JOIN-ORCHESTRATION.md) | Answers accepted 2026-11-07; implemented in software behind the closed join gate; not hardware-qualified. |
| P6-009 | Client role, restart reconciliation and observation-based leave. | [P6-009-CLIENT-RECONCILIATION.md](P6-009-CLIENT-RECONCILIATION.md) | Accepted 2026-11-09; implemented in software; not hardware-qualified. |
| P8-005 | Origin-signed link-state topology distribution. | [P8-005-TOPOLOGY-DISTRIBUTION.md](P8-005-TOPOLOGY-DISTRIBUTION.md) | Accepted as direction 2026-11-12; implementation deferred. |
| CR-F5 | Package-removal cleanup of owned networking. | [CR-F5-REMOVAL-CLEANUP.md](CR-F5-REMOVAL-CLEANUP.md) | Accepted 2026-11-13; implemented in software and isolated dpkg tests; installed-system qualification open. |

Other P4/P5/P6 documents are implementation designs, backend contracts, or acceptance procedures rather than separate ADRs. Add future decisions here when a task records an explicit adopt/reject choice.

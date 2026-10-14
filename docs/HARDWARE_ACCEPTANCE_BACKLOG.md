# BlueRoute Hardware Acceptance Backlog

**Effective 2026-10-13.** The user has deliberately postponed *new* multi-computer/physical Bluetooth tests. This deferral is **not** permission to skip required acceptance or to mark physical results passed. Software-first development continues following [TODO.md](TODO.md), with stable P0–P19 task IDs.

## Status and evidence policy

- **NOT STARTED:** task code and/or software checks remain to be written.
- **IN PROGRESS:** implementation/software qualification underway.
- **SOFTWARE COMPLETE:** merged implementation passes relevant deterministic tests and exact-SHA CI.
- **HARDWARE DEFERRED:** physical acceptance explicitly postponed; the original task stays open if hardware is part of acceptance.
- **COMPLETE:** every applicable software and physical acceptance requirement passed and is linked to evidence.
- **BLOCKED:** requires an external decision/permission/credential or another true prerequisite; merely deferred hardware does not block independent software work.

Use real Linux namespace/veth/routing tests, BlueZ and NetworkManager fakes, broker integration, simulated peers, fuzzing and fault injection where appropriate. **Simulation never demonstrates real Bluetooth radio behavior, controller peer capacity, performance, or interoperability.** Do not bypass the fail-closed P6-004 authentication guard. Preserve earlier physical evidence for the exact already-tested features.

**Required evidence for each physical closure:** task ID, date, exact source SHA and binaries/packages, machine/controller and firmware, Linux distribution/kernel, BlueZ/NetworkManager/systemd versions, PANU/NAP capabilities, topology, test commands or runbook, Wi-Fi/traffic-path isolation, raw logs and redacted diagnostics, expected/observed assertions, teardown/restart and foreign-network preservation, results, issues/remediation and links to versioned `docs/*HARDWARE-EVIDENCE*.md` records. Do not guess throughput or hardware limits.

## Prior completed hardware evidence — retained

| Tasks | Existing acceptance (limited to the tested hardware/build) |
| --- | --- |
| P4-003..P4-006 | Real BlueZ discovery, pairing, PANU/NAP/BNEP connection and cleanup; see existing P4 hardware evidence files. |
| P4-007..P4-010 | NetworkManager bridge/route, forwarding and capability-report acceptance on Debian; see existing hardware evidence files. |
| P5-006/P5-007 | Real systemd daemon behavior and read/mutation PolicyKit permission checks. |
| P6-001..P6-003 | Network creation/discovery/advertisement and explicit durable trust acceptance. Trust is *not* proof of live authenticated membership. |
| P6-005 | Two live apply/remove cycles of /24 address allocation; see [P6-005 hardware evidence](P6-005-HARDWARE-EVIDENCE-2026-09-02.md). P6-004 join remains open. |

## Deferred physical test queue

| Task(s) | Real equipment acceptance to perform later | Software development to proceed now |
| --- | --- | --- |
| P1-001..P1-004 | System/adapter inventory, discovery/pairing stability, manual PANU/NAP/IP and reproducible responsibility ADR | Reconcile earlier evidence; software/architecture work can proceed without re-running hardware. |
| P1-005..P1-007 | Real throughput, UDP loss, latency, CPU/memory, simultaneous controller capacity, range and suspend/resume | Implement benchmark/diagnostic tools and simulated faults. |
| P1-009, P16-006 | Different computer/controller classes and actual interoperability | Keep platform-independent policy; support matrix provisional. |
| P6-006 | Two-node raw Bluetooth PAN ping, SSH, 16 MiB TCP integrity, UDP payload/sequence/loss, independent DNS observation, Wi-Fi exclusion, and owner-scoped cleanup | Review/qualify/merge existing [PR #41](https://github.com/ekkus93/blueroute/pull/41) software tools; head `8a63ff64766a2c069988ee86baa1ce59d56b119c` [CI passed](https://github.com/ekkus93/blueroute/actions/runs/33694773448), not a hardware pass. Ralph Bridge `merge_pull_request` still fails with generic `INVALID_ARGUMENT` after 2026-10-13 restart; do not duplicate or bypass merge gate. Run the PR's `docs/P6-006-APPLICATION-TRAFFIC.md` when hardware returns. |
| P7-001..P7-004, P6-004 | Approved NodeId/NetworkId authenticated session and end-to-end daemon join; reject unknown/spoofed peers; rollback/cleanup on failure | P7 key-pin policy, owner-only durable key enrollment records, revocation and trust-only rejection pass software [CI 38448995440](https://github.com/ekkus93/blueroute/actions/runs/38448995440) at `897d8cdb`. Deterministic bidirectional hello/ack transcript validation also passes [CI 38455747473](https://github.com/ekkus93/blueroute/actions/runs/38455747473) at `dd51f9c` (protocol-only; not TLS authentication). Nonce-reuse rejection and deterministic TCP/TLS/hello/idle deadline policy passed [CI 38458952233](https://github.com/ekkus93/blueroute/actions/runs/38458952233) at `f4bb6906` (software models, not real TLS). Daemon startup now loads and rejects invalid key-pin state (CI [38479338847](https://github.com/ekkus93/blueroute/actions/runs/38479338847)); same-network duplicate public-key/NodeId mappings are rejected and a durable enrollment policy helper requires an independently confirmed TLS fingerprint, with negative tests ([CI 38480341965](https://github.com/ekkus93/blueroute/actions/runs/38480341965) at `36596f2f`). Strict canonical SPKI fingerprints passed [CI 38481217364](https://github.com/ekkus93/blueroute/actions/runs/38481217364) at `76cddd52`, and nested Hello unknown-field rejection passed [CI 38480899801](https://github.com/ekkus93/blueroute/actions/runs/38480899801) at `51960025`. Reconnect/loss event deduplication passed [CI 38481440066](https://github.com/ekkus93/blueroute/actions/runs/38481440066) at `dd5c71cf`. Strict nested Hello capability fields and invalid zero-capacity/battery claims now fail closed in software ([CI 38484623688](https://github.com/ekkus93/blueroute/actions/runs/38484623688) at `06e5b38a`). Incomplete control stream headers/payloads at EOF are rejected, with exact-head [CI 38490454756](https://github.com/ekkus93/blueroute/actions/runs/38490454756) at `0322101b`; this is parser software evidence only, not TLS or Bluetooth acceptance. Absolute-deadline raw TCP frame read/write and deterministic slow-peer rejection passed [CI 38495495929](https://github.com/ekkus93/blueroute/actions/runs/38495495929) at `fa6d9590`; raw TCP I/O is NOT authenticated, and the real TLS 1.3 transport remains unimplemented. Per-peer reconnect scheduling now has deterministic loss/wakeup deduplication, bounded retry exhaustion, stale-generation isolation and monotonic clock tests ([CI 38496100665](https://github.com/ekkus93/blueroute/actions/runs/38496100665) at `52b873e7`); no real reconnect transport or physical link is proven. **The policy helper is not an independently authenticated enrollment UI or actual TLS proof.** A canonical TLS exporter context and transcript session-ID equality helper passed [CI 38496556498](https://github.com/ekkus93/blueroute/actions/runs/38496556498) at `37e56d3c`; this is NOT cryptographic TLS verification or live channel binding. Bounded symmetric Hello/HelloAck claim exchange and negative tests passed [CI 38552212141](https://github.com/ekkus93/blueroute/actions/runs/38552212141) at `eaf5c0a2`; only caller-owned deadline-aware byte streams are exercised, without TLS peer or key proof. A two-peer localhost TCP integration test with mismatch/reflection negative cases passed [CI 38552630252](https://github.com/ekkus93/blueroute/actions/runs/38552630252) at `0ee2accb`; localhost is NOT Bluetooth PAN or TLS evidence. Next implement real TLS 1.3 peer verification, independently user-approved enrollment UI and service, authenticated hello/session lifecycle with cancellable I/O, then production join. No real PAN acceptance claimed. |
| P6-007, P16-002 | Multiple actual concurrent PANU clients, traffic, leave/rejoin and platform-specific peer ceiling | Simulate multiple peers/subnets/capabilities and concurrent state transitions. |
| P6-008..P6-010, P13-001..P13-006, P16-004 | Real leave, daemon/BlueZ/NetworkManager restart, adapter reset, link/range loss, suspend/resume, reconciliation without foreign-state mutation | Write software cleanup and recovery tests, deterministic fault injection, and kernel network namespace tests. |
| P8-006..P8-008, P16-003 | Bluetooth-only A–B–C two-hop IP traffic, multiple bridged-star segments, route failover, UDP/TCP performance | The P8-003 deterministic forwarding-intent model passed [CI 38478135480](https://github.com/ekkus93/blueroute/actions/runs/38478135480) at `167221e`; kernel forwarding/route application, namespaces/veth and simulated peers still need integration. Do not call it Bluetooth acceptance. |
| P9-005 | Actual hub loss/reformation over Bluetooth and restored multi-hop traffic | Test topology replanning, recovery and anti-flap in simulated heterogeneous graphs. |
| P12 UI, P16-001 | Two-node packaged installation, create/discover/join via GUI, transfer TCP/UDP with no manual networking commands, close GUI and preserve daemon link | Tauri mock and daemon/API e2e tests; package/software integration on single host. |
| P15-002..P15-005, P16-005 | Installation/upgrade/uninstall on representative machines, power/radio scanning and sustained transfer resource measurements | Debian package CI, single-host clean VM/package tests and resource instrumentation. |
| P16-007 | Actual PANU-only/client-only device can join without NAP role | Software test simulated policy restrictions. |
| P17-005, P17-006 | Final tested-support matrix, multi-controller evidence and v1 physical signoff | Draft docs and software-ready release candidate only; not final release. |

## When the computers are available

1. Prepare two compatible Linux systems (NAP host and PANU client), plus further clients for capacity/multi-hop and materially different controller hardware for P1-009/P16-006. Record complete environment and capability inventories; do not assume one Chromebook type represents all Linux hardware.
2. Recheck baseline BlueZ pairing, discovery, PANU/NAP and network ownership/cleanup. Preserve or positively isolate Wi-Fi and other route paths so Bluetooth carries the measured traffic.
3. Run P6-006's existing host/client and socket helper runbook: raw IP ping and SSH, TCP integrity, UDP loss, DNS observation as a separate assertion, and repeated cleanup. **Probe-only success does not prove production `JoinNetwork`**.
4. Qualify actual authenticated P7/P6 join, leave, reconnect, repeated daemon/system-service restarts and error cleanup using production daemon and CLI/desktop.
5. Test multi-client capacity, actual A–B–C two-hop and interconnected PAN stars, hub reformation, simulated-versus-real route accuracy and performance.
6. Test heterogeneous controllers and recovery under power cycling, adapter changes, range, suspend/resume, plus clean Debian install/upgrade/uninstall and resource usage.
7. Link versioned evidence to the original TODO entries, close *only* satisfied hardware gates, fix failures and rerun affected scenarios. **Do not pass P16 or P17-006, or claim v1/general availability, before the required evidence exists.**

## Maintenance

For each task keep independent code/CI facts (exact SHA) and hardware facts (test rig, logs, assertions). Software qualification can finish while a required hardware acceptance stays deferred. Update both this backlog and `docs/TODO.md` after later physical runs; record failures rather than silently converting tests to passes.

**Immediate next software task:** Qualify/reconcile PR #41, then build P7-001..P7-004 control sessions before completing P6-004.

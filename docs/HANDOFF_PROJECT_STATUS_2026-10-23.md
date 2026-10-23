# BlueRoute project status and continuation handoff — 2026-10-23

**Repository:** https://github.com/ekkus93/blueroute  
**Branch/policy:** Direct commits to `master` using **Ralph Bridge exclusively** for all GitHub/CI operations. Do not use default GitHub tools, and do not create a new branch/PR per task.  
**Snapshot base:** `014e56531594c3800c6f8b1d0e3aaba95f021b51` (`docs(P7-003): record multi-read operator identity consistency gates`). This is a point-in-time handoff; **reload current master, TODO, and exact-head CI before every subsequent change**.  
**Authoritative backlog:** [TODO.md](TODO.md) and [HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md). Architecture/requirements: [SPEC.md](SPEC.md).  
**Last verified successful source CI as inspected:** `3c7d881ccd722705be1955c33b48d0b408fc5490`, [run #39924091796](https://github.com/ekkus93/blueroute/actions/runs/39924091796). A newer documentation-only head may be awaiting CI; inspect its exact SHA before marking it qualified.

## Executive status

**BlueRoute is NOT merely awaiting manual hardware verification.** Its Linux Bluetooth/PAN/backend and daemon foundations are substantial and there is significant green CI, but the **end-to-end authenticated two-node create/join/use/reconnect workflow remains incomplete**, as do full routing/topology, Ratatui, Tauri desktop, continuous reconciliation, and release qualification. Many tasks in `docs/TODO.md` are `[-]` or `[ ]`. A green CI checks the implemented subset; it does **not** imply completion or physical acceptance. **Production JoinNetwork intentionally fails closed** until P7 authenticated transport exists. Physical testing is deliberately **HARDWARE DEFERRED**, not passed or blocking independent software work.

### Product and invariants

BlueRoute makes a regular IPv4 LAN over standard Bluetooth PAN/BNEP (NAP and PANU) on compatible Linux devices, later routing between PAN segments. Debian 13, BlueZ, NetworkManager, systemd and D-Bus are the initial baseline, not a product restriction to Chromebook hardware. Core logic must be hardware-agnostic; controller capacity and compatibility are discovered/tested facts. CLI, TUI and Tauri use the same daemon API. Optional Internet sharing (P18) stays **off** for initial local-network v1.

Security: Bluetooth advertisements, MAC/source-IP claims, display names and trust-only approvals are **not** authenticated membership. Bind authorization to the intended `NetworkId`, `NodeId`, independently confirmed SPKI SHA-256, actual TLS 1.3 peer-key proof, exporter-bound Hello, and current durable approval. Never bypass these checks to make a demo join succeed. Preserve foreign routes, devices, IPs and state when rolling back or reconciling.

## What is implemented and evidenced

| Tasks | Implemented / tested | Not equivalent to |
| --- | --- | --- |
| P0–P3 | Rust workspace/toolchain/format/Clippy/CI, typed domain & config, secure stable identity, durable membership/trust with v1→v2 migration. | P0-002 clean Debian acceptance and P0-004 future frontend checks. |
| P4-001..011 | BlueZ service discovery, pairing, NAP/PANU/BNEP, NetworkManager bridge/address/route/forwarding, ownership-aware cleanup and capability reports. Specific physical-hardware evidence exists for several P4 tasks. | Completed authenticated end-to-end joining. |
| P5-001..007 | Shared versioned daemon/D-Bus client, events, systemd service, least-privilege PolicyKit. Broker/daemon tests and limited hardware acceptance. | Remote authenticated member inventory or complete frontends. |
| P6-001..003 and P6-005 | Host CreateNetwork, discovery/advertisement, durable explicit trust/revocation, deterministic IPv4 segments, earlier host hardware evidence. | A production PANU member join. |
| P6-006 | Application-traffic test fixtures/runbook, simulated and localhost TCP/UDP source-isolation tests. | Actual P6-006 physical ping/SSH/TCP/UDP/DNS/Wi-Fi-exclusion acceptance, still deferred. |
| P7-001/002 | TCP-over-PAN + pinned mutual TLS 1.3 architecture and strict bounded control envelopes/replay rules. | All required live networking. |
| P7-003 | Local TLS identity generation and owner-only persistence; explicit durable peer pin policy; real rustls mutual TLS, exporter-bound authenticated Hello/HelloAck, candidate certificate inspection and confirmed-enrollment **library** helper. CLI `peer local-key` and `peer verify-local-key` perform read-only out-of-band comparison aid. | Finished independent human approval/PolicyKit ceremony and daemon enrollment orchestrator. No automatic membership from comparison. |
| P7-004 | Bounded control-session registry, retry/generation models, mutual-TLS/registered session library integration, running daemon **inbound** guarded NAP listener and worker with Heartbeat/Goodbye supervision, ownership checks, cancellation/revocation/corruption regressions. | Wired **outbound PANU** TLS/reconnect lifecycle, full end-to-end control supervision or joined client. |
| P8/P9 | Neighbor observations, subnet reservations, desired route calculation and pieces of forwarding, route diff, topology policy/host selection. | Production authenticated route distribution, live multi-hop forwarding or automatic recovery on physical PAN. |
| P10 | Clap CLI status/network list, node-local status, capability diagnostics, trust/forget, local key comparison, typed exits and version checks. | Authenticated remote peer discovery, finished join/leave UX or all remote diagnostics. |
| P14/P15 | Formal threat model, local authorization, many validation tests, Debian package manifest/build tooling, system service scripts. | Finished security review and real clean-install/upgrade/uninstall acceptance. |

### Recent committed work and CI evidence

- Post-bind durable NAP network-switch check and actual localhost socket race regression: `58f1446647961b3e69d263c659043897629969d1`, [CI #39911578707](https://github.com/ekkus93/blueroute/actions/runs/39911578707).
- Cancellation of pending daemon NAP bind now discards returned listener without subsequent observation: `8265fd48e9f711ff111155a6d4c4b7d5612971a4`, [CI #39919568152](https://github.com/ekkus93/blueroute/actions/runs/39919568152), [evidence](P7-004-CANCELLED-BIND-EVIDENCE-2026-10-23.md).
- Intentionally failing but normally ignored Rust test validates that CI detects test assertions: `0706756c37826dff9692487ca08f138ba5b861a9`, [CI #39920496143](https://github.com/ekkus93/blueroute/actions/runs/39920496143), [evidence](P0-004-CI-NEGATIVE-TEST-EVIDENCE-2026-10-23.md). Frontend checks still pending frontend existence.
- P7-003 local key comparison UX: [guide](P7-003-INDEPENDENT-KEY-COMPARISON.md), [CI #39917758815](https://github.com/ekkus93/blueroute/actions/runs/39917758815). Follow-on commits `ee4cd6b9d4bef81ee904a05841a7d894c16c7371` ([CI #39923758373](https://github.com/ekkus93/blueroute/actions/runs/39923758373)) reject zero networks/API-version drift; `3c7d881ccd722705be1955c33b48d0b408fc5490` ([CI #39924091796](https://github.com/ekkus93/blueroute/actions/runs/39924091796)) re-reads local status after diagnostics to catch daemon identity transitions. These are *comparison safety*, not full enrollment.
- Source status rows in older parts of [TODO.md](TODO.md) sometimes predate newer implementations and say that mutual TLS or daemon inbound listener is absent. Prefer the current code, later dated evidence, and **explicit remaining acceptance gates** to historical paragraphs; do not overstate either completed work or remaining work.

## Dependency-first remaining engineering (software first)

### Milestone A — first truly usable, secure two-node Bluetooth LAN

**A1. P7-003: operator-confirmed cryptographic enrollment [IN PROGRESS].** Add a complete user-facing flow for inspecting untrusted remote certificate candidate and NodeId/SPKI, obtaining *independent* human comparison/confirmation, expiration/cancellation, and PolicyKit-authorized daemon persistence. Feed the decision to existing `enroll_confirmed_control_candidate` with current P6-003 approval, a fresh pinned TLS 1.3 proof of private key, exporter-bound Hello/HelloAck, durable recheck, and strict no-silent-key-rotation. Test wrong/mismatched/corrupt/expired/revoked/reapproved keys, network drift, stale confirmation, concurrent requests, denial and restart. Read `apps/daemon/src/enrollment_transport.rs`, `control_tls.rs`, `peer_pin_store.rs`, `peer_trust.rs`, and `apps/cli/src/main.rs`. The current read-only `verify-local-key` must **never** be mistaken for authorization.

**A2. P7-004: fully operational daemon transport [IN PROGRESS].** Extend existing daemon inbound NAP control worker with live outbound PANU-side TCP/TLS session establishment from fresh BlueRoute-owned PAN address observations; integrate authenticated session registration, per-peer generation ownership, cancellation, bounded 5s/10s/30s deadlines, Goodbyes/heartbeats, revocation, lost links, retry watchdog/backoff, adapter/NM/BlueZ change, and predictable teardown/restart. Avoid blocking the D-Bus executor. Exercise daemon/process-level local TCP and fake PAN tests for success, hostile frames, duplicate sessions, stale events, lost network, and peer recovery. Relevant code: `control_worker.rs`, `control_listener.rs`, `approved_transport.rs`, `registered_transport.rs`, `attempt_socket.rs`. The partial inbound worker is real; outward PANU/reconnect ownership is not yet complete.

**A3. P7-005, P14-003/004: integrated authentication/parser hardening [IN PROGRESS].** Finish malformed/replayed/wrong-network/wrong-key/unsupported-message hostile-peer acceptance against the *running authenticated daemon*, not only synthetic parser or TLS helper; confirm no crash, no membership elevation, no leaked session, durable revocation and strict route/topology authorization. Review logs and diagnostics for secret disclosure.

**A4. P6-004: production JoinNetwork [IN PROGRESS; guard must remain closed until A1/A2].** Compose trustworthy host selection, real PANU link, automatic owner-scoped IPv4 lease, authenticated P7 control session, then durable member state **only on fully successful networking**. Implement reverse-order rollback, idempotency, failure recovery, concurrent-join serialization, accurate error reporting, membership integrity. `apps/daemon/src/join_network.rs` already has a transaction/coordinator, injected runtime fakes, and fail-closed production preflight. Do not mark tests that merely connect a PANU or configure IP as an authenticated join.

**A5. P6-008/009/010: leave/restart/reconnect [PARTIAL/OPEN].** Complete joined *client* PANU/TLS/IP leave, ownership recovery across daemon restart, desired state restoration for joiner role, BlueZ/NM loss and return, radio loss/recovery, health transitions and bounded backoff. Host leave/startup host reconciliation partially work; never silently clear membership with unknown runtime ownership. Test teardown scoped to owned resources.

**Milestone A software exit:** A1–A5 implemented and validated by real TLS/local fake PAN/namespace tests, no bypass of authorization. **Milestone A physical exit (deferred):** two supported Linux computers create, discover, verify/enroll, join and carry ordinary TCP/UDP **over Bluetooth** without manual PAN/IP commands, persist independently of UI, recover and leave without foreign-network damage. See first-useful-milestone definition in TODO. Software green is NOT physical acceptance.

### Milestone B — full local-network v1

| Tasks | Remaining deliverables |
| --- | --- |
| P6-007 | Live authenticated NAP client attach/detach, per-radio capacity and multiple concurrent client ownership. |
| P8-002/003/005–008 | Privileged route/forwarding reconciliation, authenticated route distribution, foreign route safety, namespace/veth multi-hop and interconnected-star TCP/UDP tests, then Bluetooth physical acceptance. P8-004 route calculation exists. |
| P9-001..006 | Capability-aware topology policy and host selection, plan diff/executor, hysteresis, hub-loss recovery, alternate route and simulated failure acceptance. |
| P10-002..006 | Remote authenticated node discovery/reachability, full CLI trust/enroll/network lifecycle, diagnostics, version/typed error integration. |
| P11-001..006 | Ratatui TUI event loop, subscribed state, overview/networks/devices/diagnostics, resize/reconnect, usable keyboard actions. Not yet delivered. |
| P12-001..012 | Tauri 2 desktop (React/TypeScript preferred), safe daemon bridge, first-run/platform screens, enrollment/create/join/leave/dashboard/settings/diagnostics, accessibility and fake-daemon e2e tests. Current shell is a placeholder. |
| P13-001..009 | Continuous desired-vs-observed reconciliation, BlueZ/NM restarts, adapter reset, suspend/resume, bounded failure/retry policies, structured logs and useful snapshots. |
| P14-003..006 | Integrated inter-node/authentication/security and persistence/privilege reviews for enabled functionality. |
| P15-002..005 | Debian clean install/service start, older-state upgrade/migration, uninstall/purge and owned-route/network cleanup tests; package build is not installation acceptance. |
| P17-001..006 | User/admin guides, verified support matrix and troubleshooting, security and release gates including physical/heterogeneous hardware evidence. |

**Outside first v1 unless reprioritized:** P18 Internet sharing/gateway/NAT/DNS and P19 IPv6/QR enrollment, more distros/backends and enhancements. Never accidentally enable Internet sharing as a shortcut.

## Deferred hardware/manual acceptance

[HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md) remains the source of truth. Preserve historic P4/P5/P6 evidence for the exact tested devices/builds; don't extrapolate it to production authenticated join. When available, collect (1) two-node LAN, explicit independent key approval, end-to-end PAN TCP/UDP, DNS/SSH and Wi-Fi exclusion; (2) restart/adapter/range/suspend/resume recovery; (3) simultaneous-client limits, performance, heterogeneous controllers, multi-hop stars; (4) clean Debian package install/upgrade/uninstall. Attach exact SHA and binaries, OS/kernel/BlueZ/NetworkManager/firmware, raw/redacted diagnostics, owner-scoped cleanup and test methodology. **P1/P6/P7/P8/P9/P13/P15 physical and P16 remain open** until tested.

## Workflow and continuation instructions

1. Ralph Bridge: retrieve current `master` exact head, latest CI outcome, TODO, hardware backlog and existing branches/open PRs. **No default GitHub tool** and **no new per-task branch/PR**. Confirm source SHA with every write, reconcile if another writer advanced `master`.
2. Existing [PR #41](https://github.com/ekkus93/blueroute/pull/41) (`P6-006_application_traffic`) is stale/conflicting; needed software acceptance fixtures were already reconciled into `master` at `eca4a3bed8d29d484f8ac35b46601580fbe3cea6`. Do not merge it over the newer code or duplicate it.
3. Follow TODO's dependency-first order. Next implement **A1 enrollment vertical slice**, then **A2 outbound transport/reconnect**, followed by A3/A4/A5. Prefer integration progress toward a working node pair over repeatedly adding unrelated low-risk microtests.
4. Required software gates: `cargo fmt --all -- --check`; `cargo check --workspace --locked`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo test --workspace --locked`; `python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v`, plus workflow's explicit `dbus-run-session` ignored daemon/client/CLI integration checks, packaging/systemd validations, and expected-test-failure probe. CI is defined in `.github/workflows/ci.yml`.
5. After a successful commit to `master`, verify exact-head CI; fix test/compiler/format errors, update task checkboxes only when acceptance is achieved, and keep `SOFTWARE COMPLETE / HARDWARE DEFERRED` separate from full `COMPLETE`. A source write/push alone is not passing CI.
6. Stop only on a **real** decision, credential, physical hardware requirement, or other human-only blocker. Ordinary red CI, test failures, branch movement and software dependencies are work to reconcile, not reasons to claim all development is done.

**Handoff bottom line:** Enough groundwork exists to concentrate on the first fully authenticated two-node join milestone. **Not** ready to switch to manual verification only; after that milestone there is still a large v1 backlog, especially routing, topology, user interfaces, continuous recovery and packaging.

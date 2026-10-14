# BlueRoute TODO

**Project:** BlueRoute  
**Repository:** `ekkus93/blueroute`  
**Companion specification:** `docs/SPEC.md`  

This file is the implementation backlog for BlueRoute. Task IDs are intended to remain stable so individual units of work can be referenced in issues, commits, pull requests, and development sessions.

## Conventions

- `[ ]` not started
- `[-]` in progress
- `[x]` complete
- `[!]` blocked
- The **Current task status** table is authoritative for task-level state.
- Subtask checkboxes record implementation progress. A task can remain `[-]` after all implementation subtasks are `[x]` when acceptance criteria are still outstanding.
- A task is complete only when its acceptance criteria are satisfied. Software tests do not prove physical Bluetooth behavior.
- Status notes use **NOT STARTED**, **IN PROGRESS**, **SOFTWARE COMPLETE**, **HARDWARE DEFERRED**, **COMPLETE**, or **BLOCKED** in addition to the existing checkbox syntax. Tasks with required physical acceptance cannot be `[x]` based only on CI.
- Hardware-dependent tasks must record the computer/adapter, Linux distribution, kernel, BlueZ, network-backend versions, and evidence collected.
- Do not claim physical Bluetooth behavior from CI-only tests.
- Do not generalize one adapter's limitations into a global BlueRoute limit.
- Keep architectural invariants in `docs/SPEC.md` intact unless the specification is deliberately revised first.

## Software-first development policy (2026-10-13)

**User decision:** Defer all *new* physical/multi-computer Bluetooth qualification. Continue software implementation, static checks, tests and simulated networking without waiting for equipment. Preserve existing hardware evidence exactly as recorded; don't reclassify prior results or extrapolate them to later features. Physical acceptance remains required before the first useful end-to-end milestone, hardware support claims, or v1 release.

**Execution authority:** Follow [Current dependency-first execution order](#current-dependency-first-execution-order) below rather than strictly increasing task numbers. Preserve task IDs, acceptance conditions and security invariants in `docs/SPEC.md`. **P7-001..P7-004 must precede production P6-004 completion**; never bypass authenticated NodeId/NetworkId verification to make simulated join succeed.

**Ralph Loop procedure — at startup and after every successful merge:**

1. Read the current `master` TODO, `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`, exact head, relevant active branches/PRs and CI through **Ralph Bridge**, and reconcile already-written work before implementing more.
2. Prioritize reviewing and qualifying existing [PR #41](https://github.com/ekkus93/blueroute/pull/41). Avoid per-task branches/PRs; use small, cohesive direct-to-`master` commits via Ralph Bridge where authorized. Never bypass configured write permissions or branch protection.
3. Validate against the exact commit: format, Clippy, unit/protocol tests, fake BlueZ/NetworkManager, real-broker daemon tests, negative-authentication tests and simulated Linux network namespaces/veth when runner privileges permit. Offer unprivileged deterministic substitutes in CI. Use fault injection for restart/loss/rollback; virtual Ethernet cannot prove Bluetooth radio operation.
4. Diagnose ordinary CI failures and continue useful tasks during bounded CI waits; a timeout, lint failure or transient error does not itself require user help.
5. When code and software tests pass but the task's actual physical criteria remain, record **SOFTWARE COMPLETE / HARDWARE DEFERRED** with commit, CI, test plan and evidence needs in the backlog. Leave the original hardware-dependent acceptance checklist unchecked and continue to the next software task. Do not call the task fully COMPLETE or claim that physical PAN/throughput/multi-hop worked.
6. Stop only for a real decision, authorization, credential, hardware-dependent *final acceptance*, or exhaustion of implementable software tasks. The deliberate physical deferral alone must never block independent development.

**Separate acceptance tracker:** [HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md).

## Current task status

**Status date:** 2026-10-13 (P7 security/software changes qualified through `06e5b38a`; PR #41 remains separately inspected and unmerged)

| Task | Status | Current state |
| --- | --- | --- |
| P0-001 | `[x]` | Workspace structure is complete and `cargo check --workspace --locked` is green on `master`. |
| P0-002 | `[-]` | Toolchain/development policy documented; clean supported Debian validation pending. |
| P0-003 | `[x]` | Rust formatting/lint policy is configured; `cargo fmt --all -- --check` and locked Clippy with `-D warnings` are green on `master`. |
| P0-004 | `[-]` | CI runs on pull requests and `master`; green master execution and formatting-failure detection are proven, while an intentional test-failure probe is still pending. |
| P0-005 | `[x]` | Development documentation complete. |
| P1-001..P1-007 | `[ ]` | **HARDWARE DEFERRED** for remaining characterization; preserve relevant prior hardware evidence, but do not claim these specific P1 criteria satisfied without linked results. Software can proceed. |
| P1-008 | `[x]` | Capability-matrix format complete. |
| P1-009 | `[ ]` | **HARDWARE DEFERRED** pending a materially different Bluetooth controller/system class. |
| P2-001..P2-008 | `[x]` | Domain implementation and tests are complete; the full locked workspace test suite is green on `master`. |
| P3-001 | `[x]` | Versioned hardware-agnostic configuration schema and validation are implemented and green in CI. |
| P3-002 | `[x]` | Stable node identity generation/persistence is wired into daemon startup; owner-only storage and restart recovery tests are green in CI. |
| P3-003 | `[x]` | Durable known-network and peer membership/trust state persists through restart; forget/cleanup and corruption handling are tested and green in CI. |
| P3-004 | `[x]` | Membership persistence uses an explicit v2 schema with tested in-place v1 migration; unsupported old/future schemas fail closed without rewriting state. |
| P3-005 | `[x]` | Persistent-secret policy, redacting secret wrapper, and restrictive Linux secret storage are implemented with permission/redaction tests. |
| P4-001 | `[x]` | Backend-neutral adapter boundaries and fake-backend tests are implemented and green in CI. |
| P4-002 | `[x]` | Direct system-D-Bus BlueZ service/adapter discovery, Powered-state mapping, and adapter-change subscriptions are implemented and green in locked CI; physical-controller validation remains in P1. |
| P4-003 | `[x]` | BlueZ discovery lifecycle, Device1 mapping, peer events, and real nearby-Linux-node hardware acceptance are complete; `debiancb1` was observed through the Rust adapter. |
| P4-004 | `[x]` | BlueZ pairing/trust, Rust-controlled Agent1 callbacks, typed rejection/timeout handling, and real two-node Rust-controlled hardware acceptance are complete. |
| P4-005 | `[x]` | BlueZ Network1 PANU connect/interface mapping (including udev rename reconciliation), bounded connect timeout/cancellation, loss observation, idempotent disconnect, and real two-node PANU data-plane hardware acceptance are complete. |
| P4-006 | `[x]` | BlueZ NetworkServer1 NAP registration, client attach/detach observation, ownership-aware idempotent cleanup, typed capability failures, and real two-node Rust NAP/PANU data-plane hardware acceptance are complete. |
| P4-007 | `[x]` | Direct NetworkManager system-D-Bus backend, bounded state observation, explicit ownership, BlueRoute-owned bridge/address lifecycle, fail-closed cross-owner handling, and real Debian 13 hardware acceptance are complete. |
| P4-008 | `[x]` | NetworkManager route inspection/add-update-remove, ownership enforcement, fresh-backend reconciliation, idempotent cleanup, foreign/default-route preservation, and live Debian 13 kernel-route hardware acceptance are complete. |
| P4-009 | `[x]` | Node-global IPv4 forwarding control, boot-local ownership lease, fresh-backend reconciliation, idempotent release, foreign forwarding preservation, NAT/firewall separation, and live kernel hardware acceptance are complete. |
| P4-010 | `[x]` | Read-only system capability reporting, support classification, runtime diagnostics, and live Debian hardware acceptance are complete. |
| P4-011 | `[x]` | Reusable fake-backend contract tests, NetworkManager trait conformance, and core/topology architecture guards are implemented and green in CI. |
| P5-001 | `[x]` | Versioned local API contract and real-broker client enforcement are complete; incompatible daemon replacement is rejected before normal commands. |
| P5-002 | `[x]` | Shared command/response types now have deterministic validated JSON serialization, comprehensive round-trip tests, and fail-closed malformed-payload coverage. |
| P5-003 | `[x]` | Event model and deterministic structural tests are implemented and green in CI. |
| P5-004 | `[x]` | Versioned daemon D-Bus service ownership, status/capability queries, fail-closed request decoding, typed event signaling, and real broker-level CI acceptance are complete. |
| P5-005 | `[x]` | Shared system-D-Bus client, mandatory version gating, typed requests/events, bounded reconnect, and real-broker restart/incompatibility acceptance are complete. |
| P5-006 | `[x]` | System-level systemd service packaging, D-Bus name ownership, bounded restart policy, journald integration, boot activation, and GUI-logout independence are proven on Debian 13 hardware. |
| P5-007 | `[x]` | Least-privilege D-Bus/PolicyKit authorization is implemented and live Debian acceptance proves unprivileged read-only inspection plus fail-closed unauthorized mutation denial. |
| P6-001 | `[x]` | CreateNetwork establishes durable member state, a deterministic BlueRoute-owned NetworkManager bridge/address, a live BlueZ NAP, and real PANU attachment on physical hardware. |
| P6-002 | `[x]` | Hosted networks advertise the full logical `NetworkId` through BlueZ LE manufacturer data; `arisu` discovered the exact `debiancb1` network without manual MAC entry, with discovery metadata explicitly non-authoritative for security. |
| P6-003 | `[x]` | Explicit BlueRoute peer approval/revocation is PolicyKit-gated and durable; live acceptance persisted `arisu` as trusted but non-member, proved idempotent reapproval, and removed the approval without disturbing network state. |
| P6-004 | `[-]` | **IN PROGRESS**: transaction coordinator, rollback and PolicyKit authorization are merged. Production join preflight intentionally fails closed pending P7 authenticated control sessions, PANU/control wiring, and physical end-to-end acceptance. |
| P6-005 | `[x]` | **COMPLETE** in `master` at `c2db781`. Deterministic RFC1918 subnet, live conflict detection, NAP/PANU addresses, cleanup and two real NetworkManager apply/remove cycles; see `docs/P6-005-HARDWARE-EVIDENCE-2026-09-02.md`. |
| P6-006 | `[-]` | **IN PROGRESS**: existing open [PR #41](https://github.com/ekkus93/blueroute/pull/41) contains qualified software probes at `8a63ff6` ([CI passed](https://github.com/ekkus93/blueroute/actions/runs/33694773448)). Ralph Bridge merge endpoint rejects both `merge` and `squash` with generic `INVALID_ARGUMENT` even after 2026-10-13 restart. Do not duplicate/bypass the PR; reconcile through authorized merge when available. Real ping/SSH/TCP/UDP/cleanup **HARDWARE DEFERRED**. |
| P7-001 | `[x]` | **SOFTWARE COMPLETE (design only)**: transport/authentication ADR chooses control TCP-over-PAN and explicitly pinned mutual TLS 1.3. P7-003/004 are required before any production join. |
| P7-002 | `[x]` | **SOFTWARE COMPLETE**: strict bounded frame codec/replay tracker; [CI 38397384422](https://github.com/ekkus93/blueroute/actions/runs/38397384422) passed at `63e8cf7`. It does not perform peer authentication. |
| P7-003 | `[-]` | **IN PROGRESS**: typed validated hello/capability/topology claims plus fail-closed `(NetworkId,NodeId,SPKI-SHA256)` key pinning, secure owner-only durable enrollment records and daemon approval checks. Peer revocation erases persisted pin; reapproval does not restore it. [CI 38448995440](https://github.com/ekkus93/blueroute/actions/runs/38448995440) passed at `897d8cdb`. Daemon startup validates and wires owner-only peer-key pins ([CI 38479338847](https://github.com/ekkus93/blueroute/actions/runs/38479338847)); duplicate key-to-NodeId aliases are rejected and independent fingerprint confirmation is required by the durable enrollment policy helper ([CI 38480341965](https://github.com/ekkus93/blueroute/actions/runs/38480341965) at `36596f2f`). Strict canonical SHA-256 fingerprint parsing/display is shared between protocol and persistence ([CI 38481217364](https://github.com/ekkus93/blueroute/actions/runs/38481217364) at `76cddd52`). Real TLS identity verification, authenticated hello exchange and key enrollment UX still missing. |
| P7-004 | `[-]` | **IN PROGRESS**: bounded peer registry/stale-event isolation and bounded exponential reconnect policy passed exact-head [CI 38444858686](https://github.com/ekkus93/blueroute/actions/runs/38444858686) at `c688f799`. Duplicate loss notifications and repeated reconnect-state wakeups are now idempotent ([CI 38481440066](https://github.com/ekkus93/blueroute/actions/runs/38481440066) at `dd5c71cf`). Raw TCP frame I/O now has absolute whole-frame read/write deadlines, timeout restoration, and slow-peer/expired-deadline tests ([CI 38495495929](https://github.com/ekkus93/blueroute/actions/runs/38495495929) at `fa6d9590`). This is transport groundwork, NOT authenticated TLS. Key-pinned peer verification, TLS I/O, reconnection driver, and daemon integration remain unimplemented. |
| P7-005 | `[-]` | **IN PROGRESS**: parser fuzz-style deterministic mutation, frame truncation, version incompatibility and round-trip tests passed exact-head [CI 38399834487](https://github.com/ekkus93/blueroute/actions/runs/38399834487) at `964b7add`. Unknown fields within Hello message variants fail strict parsing ([CI 38480899801](https://github.com/ekkus93/blueroute/actions/runs/38480899801) at `51960025`). Strict nested capability claims and invalid zero-capacity/battery claims also fail closed ([CI 38484623688](https://github.com/ekkus93/blueroute/actions/runs/38484623688) at `06e5b38a`). Incremental frame decoder rejects EOF on any partial header/body; exact-head [CI 38490454756](https://github.com/ekkus93/blueroute/actions/runs/38490454756) passed at `0322101b`. Daemon wire integration/fuzz acceptance still depends on P7-004. |
| P10-001 | `[-]` | **IN PROGRESS:** early read-only CLI uses the version-gated `blueroute-client` and has `--help`/`--version`, preliminary usage and daemon error exits; **clap**, complete exit taxonomy and further commands remain. Exact-head [CI 38461039721](https://github.com/ekkus93/blueroute/actions/runs/38461039721) passed at `51160db`. |
| P10-002 | `[-]` | **IN PROGRESS:** `status`, `capability show` and `network list` support human/JSON output through the typed D-Bus client (exact-head [CI 38463280247](https://github.com/ekkus93/blueroute/actions/runs/38463280247)); node list/show remains unimplemented. |
| P14-001 | `[x]` | **COMPLETE (threat model only):** `docs/P14-001-THREAT-MODEL.md`, [CI 38466101723](https://github.com/ekkus93/blueroute/actions/runs/38466101723); implementation safeguards and physical acceptance remain separate. |
| P8-001 | `[x]` | **SOFTWARE COMPLETE (observation model):** `NeighborObservations` keeps expiring untrusted discovered candidate peers separate from active observed `TopologyGraph` PAN links, with bounded capacity, monotonic timestamp ordering and capability annotations; [CI 38469158021](https://github.com/ekkus93/blueroute/actions/runs/38469158021) passed at `56208e3`. Live BlueZ/P7 ingestion remains P8 integration, not evidence of RF behavior. |
| P8-002 | `[-]` | **IN PROGRESS:** deterministic non-overlapping multi-segment private IPv4 allocation, host-prefix collision checks and owner-scoped *reservation* release passed [CI 38470193934](https://github.com/ekkus93/blueroute/actions/runs/38470193934) at `2c2c108`. A regression fix now revalidates newly observed foreign routes before reusing an existing reservation; exact-head CI 38477587852 passed at `ed628447`. Live kernel route/address ownership teardown and actual PAN segment integration remain; no physical hardware claim. |
| P8-003 | `[-]` | **IN PROGRESS:** deterministic fail-closed multi-PAN forwarding intent requires explicitly routing-capable local node and active links in at least two distinct PAN segments; [CI 38478135480](https://github.com/ekkus93/blueroute/actions/runs/38478135480) passed at `167221e`. Actual privileged forwarding reconciliation, owner-scoped lease and kernel network namespace qualification remain. |
| P8-004 | `[x]` | **SOFTWARE COMPLETE (desired-route calculation):** deterministic weighted node routes over active PAN graph respect explicitly routing-capable transit nodes, stable healthy paths and partitions; [CI 38471055209](https://github.com/ekkus93/blueroute/actions/runs/38471055209) passed at `32d4954`. Applying/distributing privileged routes remains P8-005 and physical multi-hop tests remain deferred. |
| P14-004 | `[-]` | **IN PROGRESS:** shared `DisplayName` rejects control/escape and Unicode bidirectional-override characters while preserving printable international names; unit tests pass [CI 38467939586](https://github.com/ekkus93/blueroute/actions/runs/38467939586) at `939935e`. Full peer-field limits, unsupported messages and fuzz/domain conversion review still outstanding. |
| P10-003 | `[-]` | **IN PROGRESS:** network creation and discovery start/stop delegate to PolicyKit-protected typed daemon API; parsing/validation tests pass [CI 38466994092](https://github.com/ekkus93/blueroute/actions/runs/38466994092) at `c786f77`. Production join/leave and full end-to-end lifecycle remain. |
| P10-004 | `[-]` | **IN PROGRESS:** explicit full-NodeId `peer trust`/`peer forget` invoke existing PolicyKit-protected D-Bus commands; tests and [CI 38466585585](https://github.com/ekkus93/blueroute/actions/runs/38466585585) at `8183225` passed. Pending requests/authenticated-join UX remain; trust is **not** TLS enrollment or membership. |
| All other tasks | `[ ]` | Not started unless explicitly documented in that task; remaining hardware acceptance is not a blocker for independent software work. |

## Platform rule

BlueRoute targets **compatible Linux computers with Bluetooth PAN support**, not Dell Chromebook 3100 specifically. Dell Chromebook 3100 machines running Debian are available early test fixtures and should be used for useful evidence, but implementation code and product policy must remain hardware-agnostic.

The initial software/packaging baseline is Debian 13 with BlueZ, NetworkManager, systemd, and system D-Bus. NetworkManager is an initial backend, not a permanent core dependency.

## Release strategy

1. Establish the Rust/project baseline.
2. Prove Bluetooth PAN/BNEP on available Linux hardware and characterize capability differences.
3. Build hardware-independent Rust domain models and Linux adapter boundaries.
4. Make a daemon own the networking lifecycle.
5. Deliver a reliable single-star BlueRoute LAN.
6. Add authenticated control and routed interconnected-star topology.
7. Add automatic topology management driven by capabilities rather than device models.
8. Deliver CLI, TUI, and polished Tauri front ends around the same daemon API.
9. Harden, package, and validate across representative Linux/Bluetooth hardware.
10. Add optional Internet gateway support after local routed networking is solid.

---

# P0 — Repository and engineering baseline

## P0-001 — Define Rust workspace

- [x] Create root Cargo workspace.
- [x] Add:
  - `crates/blueroute-core`
  - `crates/blueroute-protocol`
  - `crates/blueroute-linux`
  - `crates/blueroute-client`
  - `apps/daemon`
  - `apps/cli`
  - `apps/tui`
  - `apps/desktop`
- [x] Build placeholder crates.

**Acceptance**

- `cargo check --workspace` succeeds.
- Shared domain types are not duplicated in applications.

## P0-002 — Pin development toolchain policy

- [x] Add `rust-toolchain.toml` or documented minimum stable Rust.
- [x] Document Debian development dependencies.
- [x] Document Node/package-manager requirements when Tauri frontend work begins.
- [x] State clearly that Debian is the initial development baseline, not the only architectural target.

**Acceptance**

- A clean supported Debian machine can run documented checks.

## P0-003 — Establish formatting and linting

- [x] Configure rustfmt.
- [x] Configure clippy policy.
- [ ] Add frontend formatting/linting when frontend exists. *(Deferred until frontend work begins.)*

**Acceptance**

- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets` passes under agreed policy.

## P0-004 — Establish CI baseline

- [x] Add GitHub Actions for formatting, clippy, tests, and build.
- [ ] Add frontend checks when introduced. *(Deferred until frontend work begins.)*
- [x] Cache dependencies without masking lockfile errors.

**Acceptance**

- CI runs on pull requests and `master`.
- Deliberate formatting/test failures are detected.

## P0-005 — Add development documentation

- [x] Add contributor quick start.
- [x] Explain deterministic tests vs physical Bluetooth tests.
- [x] Document task-ID convention.
- [x] Document platform-support terminology: supported, tested, experimental, unsupported.

**Acceptance**

- Contributors understand that one successful computer model does not establish a universal hardware claim.

---

# P1 — Linux Bluetooth/PAN capability characterization

Do this early. Architecture must be informed by real Bluetooth PAN behavior, but it must characterize capabilities across systems rather than become specialized to one model.

## P1-001 — Inventory initial test systems

- [ ] Record every available Linux test machine used for early work.
- [ ] Record vendor/model only for reproducibility.
- [ ] Record Bluetooth controller/chipset identifiers.
- [ ] Record kernel driver/module and firmware information where useful.
- [ ] Record distribution, kernel, BlueZ, NetworkManager, and systemd versions.
- [ ] Capture relevant `bluetoothctl show`, `btmgmt info`, `lsusb`/`lspci`, and network information.
- [ ] Include Dell Chromebook 3100 units as initial fixtures if available.

**Acceptance**

- Versioned reports exist under `docs/hardware/` or `docs/platforms/`.
- Reports describe capability evidence, not product requirements.

## P1-002 — Verify basic Bluetooth stability

- [ ] Confirm adapter power state changes reliably.
- [ ] Confirm discovery.
- [ ] Pair two compatible Linux nodes.
- [ ] Disconnect/reconnect repeatedly.
- [ ] Test after reboot.
- [ ] Record driver/firmware quirks.

**Acceptance**

- Reproducible pairing/reconnection procedures and results exist for the tested combination.

## P1-003 — Prove manual PANU/NAP connectivity

- [ ] Create one NAP and one PANU with existing Linux tooling/APIs.
- [ ] Confirm BNEP interface creation.
- [ ] Assign working IPv4 configuration.
- [ ] Ping both directions.
- [ ] Run TCP traffic.
- [ ] Run UDP traffic.
- [ ] Disable Wi-Fi during acceptance to prove the data path.

**Acceptance**

- Two compatible Linux nodes exchange ordinary TCP/IP over Bluetooth PAN.
- Exact hardware/software combination and commands are documented.

## P1-004 — Determine NetworkManager vs direct BlueZ responsibility

- [ ] Prototype PAN setup through NetworkManager.
- [ ] Prototype/inspect BlueZ network APIs where relevant.
- [ ] Record ownership of NAP registration, PANU connection, IP configuration, and teardown.
- [ ] Write an ADR.

**Acceptance**

- Production boundary is chosen based on reproducible Linux testing, not Chromebook-specific behavior.

## P1-005 — Measure single-link baseline

- [ ] TCP throughput.
- [ ] UDP behavior.
- [ ] latency/loss.
- [ ] CPU/memory.
- [ ] sustained transfer.
- [ ] practical distances/radio conditions.

**Acceptance**

- Results include complete platform/adapter/version metadata.

## P1-006 — Measure simultaneous PAN connection limits

- [ ] Add PANU clients one at a time.
- [ ] Exercise simultaneous traffic.
- [ ] Record stable and unstable counts.
- [ ] Record failure modes.
- [ ] Repeat on more than one adapter/controller class as hardware becomes available.

**Acceptance**

- BlueRoute has per-platform evidence and a conservative capability-policy approach.
- No global peer-count constant is inferred from one controller.

## P1-007 — Test suspend/resume and range loss

- [ ] Suspend NAP and PANU where platform supports suspend.
- [ ] Resume and inspect BlueZ/network state.
- [ ] Move a node out of range and back.
- [ ] Record stale interfaces/profiles/routes.

**Acceptance**

- Recovery requirements are documented without assuming every Linux node has identical suspend semantics.

## P1-008 — Create capability matrix format

- [x] Define fields for PANU, NAP, forwarding, practical connection limit, backend, and known quirks.
- [x] Distinguish discovered, measured, configured, and conservative-default capabilities.
- [x] Define how unknown capability is represented.

**Acceptance**

- Two different hardware profiles can be represented without code changes.

## P1-009 — Add second hardware-class validation

- [ ] Test at least one materially different Linux/Bluetooth system from the initial Chromebook fixtures before broad v1 portability claims.
- [ ] Repeat basic discovery, pairing, PANU/NAP, and traffic tests.
- [ ] Compare behavioral differences.

**Acceptance**

- Architecture assumptions that accidentally depend on one controller are identified before v1 support claims.

---

# P2 — Core domain model

## P2-001 — Define stable identifiers

- [x] `NodeId`.
- [x] `NetworkId`.
- [x] link/segment identity if needed.
- [x] Separate human-readable names from authorization identity.

**Acceptance**

- IDs serialize deterministically.
- Display-name changes do not change identity.
- Parsing/serialization tests exist.

## P2-002 — Define node capability model

- [x] Bluetooth adapter usability.
- [x] PANU capability.
- [x] NAP capability.
- [x] routing capability.
- [x] network-backend capability.
- [x] connection policy ceiling.
- [x] optional link-quality/power data.
- [x] reserve external-connectivity/gateway fields.
- [x] distinguish `has_internet` from `willing_to_share_internet`.
- [x] track capability source where useful.

**Acceptance**

- Core can represent heterogeneous Linux nodes without model-specific types.

## P2-003 — Define network membership model

- [x] local membership state.
- [x] trusted/known peers.
- [x] network name vs identity.
- [x] join/leave transitions.

**Acceptance**

- Tests cover legal/illegal transitions.

## P2-004 — Define link and PAN-segment model

- [x] Represent PANU/NAP relationship.
- [x] Represent link state/health/properties.
- [x] Keep a PAN segment separate from the logical BlueRoute network.

**Acceptance**

- Single-star and routed multi-star graphs are representable.

## P2-005 — Define topology graph

- [x] Graph operations for nodes/links.
- [x] Direct vs routed reachability.
- [x] Failed/unavailable links.
- [x] Deterministic snapshots.

**Acceptance**

- Tests cover connect/disconnect, partitions, redundant paths, and heterogeneous capabilities.

## P2-006 — Define route model

- [x] Prefix/segment destinations.
- [x] next hop and cost.
- [x] ownership metadata.
- [x] reserve default/Internet destination semantics.

**Acceptance**

- Model supports local-only v1 and future gateway routing without breaking redesign.

## P2-007 — Define health model

- [x] healthy/degraded/reconnecting/error concepts.
- [x] separate adapter, runtime prerequisite, membership, link, topology, and gateway health.
- [x] user-facing aggregation rules.

**Acceptance**

- A client-only node is not considered unhealthy merely because it lacks NAP capability.
- Tests cover representative combinations.

## P2-008 — Define typed error taxonomy

- [x] unsupported runtime.
- [x] missing/disabled adapter.
- [x] required capability unavailable.
- [x] BlueZ/backend unavailable.
- [x] pairing/auth errors.
- [x] PAN/address/route/topology errors.
- [x] protocol/state errors.

**Acceptance**

- UIs can show friendly messages while diagnostics retain low-level context.

---

# P3 — Configuration and persistence

## P3-001 — Define configuration schema

- [x] daemon settings.
- [x] display name.
- [x] address-pool policy.
- [x] topology/capability policy overrides.
- [x] network backend selection field or internal abstraction as appropriate.
- [x] gateway settings reserved and disabled.

**Acceptance**

- Configuration is versioned/validated and contains no hard-coded computer-model policy.

## P3-002 — Implement stable node identity persistence

- [x] Generate identity first run.
- [x] Persist securely.
- [x] Recover after restart.

**Acceptance**

- Reboot does not create a new logical node.

## P3-003 — Implement network membership persistence

- [x] known networks.
- [x] peer membership/trust data.
- [x] cleanup/forget semantics.

**Acceptance**

- Remembered network state survives intended restarts.

## P3-004 — Add schema migration framework

- [x] version persistent format.
- [x] migration entry points.
- [x] synthetic migration test.

**Acceptance**

- Schema evolution does not require deleting user identity/state.

## P3-005 — Secure persistent secrets

- [x] identify secrets.
- [x] restrictive permissions.
- [x] log/debug redaction.

**Acceptance**

- Tests cover redaction and permissions where feasible.

---

# P4 — Linux system adapters

## P4-001 — Create adapter trait boundaries

- [x] Bluetooth backend trait(s).
- [x] IP/network backend trait(s).
- [x] capability-discovery interfaces.
- [x] clock/event abstractions for deterministic tests.
- [x] keep core free of D-Bus types.

**Acceptance**

- Core tests run with fake adapters.
- Future non-NetworkManager backend can fit the interface without changing topology types.

## P4-002 — Implement BlueZ service/adapter discovery

- [x] Rust system-D-Bus connection.
- [x] enumerate adapters.
- [x] observe power state.
- [x] subscribe to changes.

**Acceptance**

- Probe reports adapter state without `bluetoothctl` parsing.

## P4-003 — Implement Bluetooth device discovery adapter

- [x] start/stop discovery.
- [x] device add/change/remove.
- [x] map properties into domain types.
- [x] bound discovery state.

**Acceptance**

- Nearby compatible Linux test nodes appear through Rust adapter.
- Hardware acceptance is recorded in `docs/P4-003-HARDWARE-EVIDENCE-2026-08-29.md`; the known Linux node `debiancb1` appeared through the Rust adapter.

## P4-004 — Implement pairing/trust adapter

- [x] initiate pairing.
- [x] handle agent/callback needs.
- [x] trust/untrust according to policy.
- [x] typed rejection/timeouts.

**Acceptance**

- Two test nodes complete pairing through Rust-controlled flow.
- Hardware acceptance is recorded in `docs/P4-004-HARDWARE-EVIDENCE-2026-08-30.md`; `arisu` paired with `debiancb1` through the Rust-controlled acceptor/initiator flow, and the initiator verified `paired=true` and `trusted=true`.

## P4-005 — Implement PANU connection adapter

- [x] establish PANU connection through selected API.
- [x] identify resulting interface.
- [x] observe loss.
- [x] idempotent disconnect.

**Acceptance**

- Hardware integration path creates a working PANU data plane on supported test hardware.
- Hardware acceptance is recorded in `docs/P4-005-HARDWARE-EVIDENCE-2026-08-30.md`; `arisu` established a Rust-controlled PANU link to `debiancb1` and carried IPv4/ICMP traffic over BNEP with 0% packet loss.
- Implementation/design notes and the hardware probe are documented in `docs/P4-005-PANU.md`.

## P4-006 — Implement NAP lifecycle adapter

- [x] register/start NAP.
- [x] accept PAN clients.
- [x] observe attach/detach.
- [x] idempotent stop/cleanup.
- [x] return capability error cleanly if local stack cannot provide NAP.

**Acceptance**

- Rust path replaces manual P1 setup for a supported NAP/PANU pair.
- Hardware acceptance is recorded in `docs/P4-006-HARDWARE-EVIDENCE-2026-08-31.md`; the Rust NAP on `debiancb1` accepted the Rust PANU from `arisu`, observed attach/detach, carried IPv4/ICMP traffic over BNEP with 0% packet loss, and completed repeated idempotent NAP teardown.
- Implementation/design notes and the hardware probe are documented in `docs/P4-006-NAP.md`.

## P4-007 — Implement NetworkManager backend

- [x] enumerate relevant connections/devices.
- [x] observe changes.
- [x] apply BlueRoute-owned addressing.
- [x] remove only BlueRoute-owned state.

**Acceptance**

- No production operation parses `nmcli` output.
- Hardware acceptance is recorded in `docs/P4-007-HARDWARE-EVIDENCE-2026-09-01.md`; the Rust NetworkManager backend on `debiancb1` enumerated live state, observed bridge/profile events, applied `10.254.89.1/30`, rejected cross-owner takeover without leaking state, completed repeated idempotent cleanup, and preserved all 15 baseline foreign NetworkManager profiles.
- Implementation/design notes and the hardware probe are documented in `docs/P4-007-NETWORKMANAGER.md`.

## P4-008 — Implement route adapter

- [x] inspect routes.
- [x] add/update/remove BlueRoute routes.
- [x] ownership identification.
- [x] reconcile after backend restart.

**Acceptance**

- Repeated application is idempotent and foreign routes are preserved.
- Implementation/design notes and the live route probe are documented in `docs/P4-008-ROUTES.md`.
- Hardware acceptance is recorded in `docs/P4-008-HARDWARE-EVIDENCE-2026-09-01.md`; the Rust NetworkManager backend on `debiancb1` installed and rediscovered `10.254.91.0/24 via 10.254.90.2 dev br-blue-rt` at metric 77 in the live kernel, reconciled a stale metric-177 variant without duplication, rejected cross-owner mutation without leaked state, completed repeated idempotent cleanup, preserved all 15 baseline foreign NetworkManager profiles, and left the pre-existing default route unchanged.

## P4-009 — Implement forwarding adapter

- [x] per-node forwarding abstraction.
- [x] enable only when routed topology requires it.
- [x] keep NAT/firewall separate for gateway phase.

**Acceptance**

- Routed implementation can enable forwarding without coupling to Internet NAT.
- Implementation/design notes and the live forwarding probe are documented in `docs/P4-009-FORWARDING.md`.
- Hardware acceptance is recorded in `docs/P4-009-HARDWARE-EVIDENCE-2026-09-01.md`; `debiancb1` completed a live kernel `0 -> 1 -> 0` forwarding lifecycle with a restrictive BlueRoute runtime lease, fresh-backend reconciliation, repeated idempotent release, lease removal, and default-route preservation, while a supplementary `arisu` run proved pre-existing forwarding remains enabled after BlueRoute releases its lease.

## P4-010 — Implement system capability report

- [x] BlueZ availability/version.
- [x] network backend/version.
- [x] Bluetooth adapters/controllers/drivers.
- [x] PANU/NAP capability observations where determinable.
- [x] forwarding capability.
- [x] practical/configured peer ceiling.
- [x] kernel/runtime prerequisites.

**Acceptance**

- Diagnostics explain why a system is fully supported, client-only, degraded, or unsupported.
- Hardware acceptance is recorded in `docs/P4-010-HARDWARE-EVIDENCE-2026-09-01.md`; `debiancb1` was correctly classified as fully supported from live BlueZ, NetworkManager, controller/driver, PANU/NAP, BNEP, forwarding, peer-ceiling, kernel, and runtime-prerequisite observations.
- Implementation/design notes and the read-only probe are documented in `docs/P4-010-CAPABILITIES.md`.

## P4-011 — Define network-backend abstraction tests

- [x] Fake backend contract tests.
- [x] NetworkManager implementation conformance tests.
- [x] Ensure topology/core never imports NetworkManager-specific types.

**Acceptance**

- A future backend can be tested against the same contract.
- The reusable contract, NetworkManager conformance strategy, and architecture guard are documented in `docs/P4-011-NETWORK-BACKEND-CONTRACTS.md` and exercised by `crates/blueroute-linux/tests/network_backend_contract.rs`.

---

# P5 — Local daemon protocol and service

## P5-001 — Define local API version

- [x] protocol version.
- [x] compatibility rules.
- [x] D-Bus service/object/interface naming.

**Acceptance**

- Client detects incompatibility before normal commands.
- Real-broker incompatibility enforcement is recorded in `docs/P5-005-CLIENT.md`; an incompatible replacement daemon was rejected with zero calls to its normal `Request` method.

## P5-002 — Define command/response types

- [x] status/capabilities.
- [x] network list/create/join/leave.
- [x] node list/get/name.
- [x] discovery.
- [x] trust/forget.
- [x] diagnostics.
- [x] reserve Internet-sharing command.
- [x] deterministic serialization/round-trip tests.

**Acceptance**

- Types live in shared protocol crate and are serialization tested.
- The stable JSON payload contract, validated identifier/name encoding, round-trip coverage, and reserved gateway behavior are documented in `docs/P5-PROTOCOL.md`.

## P5-003 — Define event types

- [x] network discovery.
- [x] peer/node changes.
- [x] capability changes.
- [x] membership.
- [x] link/topology/route.
- [x] health/authorization.
- [x] reserve Internet/gateway events.

**Acceptance**

- Event snapshots are deterministic enough for UI tests.

## P5-004 — Implement daemon D-Bus service skeleton

- [x] own service name.
- [x] expose version/status/capabilities.
- [x] emit test event.
- [x] reject malformed requests safely.

**Acceptance**

- Test client queries daemon and receives events.
- Service design, fail-closed behavior, and real-broker acceptance are documented in `docs/P5-004-DBUS-SERVICE.md`.

## P5-005 — Implement `blueroute-client`

- [x] connect.
- [x] version negotiation.
- [x] typed requests.
- [x] events.
- [x] reconnect after daemon restart.

**Acceptance**

- CLI/TUI/Tauri share the crate with no duplicate system-D-Bus networking logic.
- Client design, non-replay reconnect semantics, and real-broker acceptance are documented in `docs/P5-005-CLIENT.md`.

## P5-006 — Add systemd service

- [x] unit file.
- [x] restart policy.
- [x] startup ordering.
- [x] journald logging.

**Acceptance**

- Daemon starts at boot on supported Debian baseline and survives GUI logout.
- Hardware acceptance is recorded in `docs/P5-006-HARDWARE-EVIDENCE-2026-09-01.md`; the enabled system service started automatically after reboot, survived GUI logout/login with the same PID and InvocationID, restarted after forced `SIGKILL`, and retained restrictive systemd-managed state/runtime directories.

## P5-007 — Define D-Bus/Polkit authorization policy

- [x] read-only vs mutating operations.
- [x] authorization behavior.
- [x] no blanket front-end root access.

**Acceptance**

- Intended users can inspect status; unauthorized sensitive changes fail safely.
- Hardware acceptance is recorded in `docs/P5-007-HARDWARE-EVIDENCE-2026-09-01.md`; the normal Debian user read version/status without elevation, an unprivileged `nobody` mutation was denied before dispatch with a nonzero D-Bus call result, and the daemon remained active.

---

# P6 — Single-star BlueRoute LAN

This is the first complete product slice.

## P6-001 — Implement create-network operation

- [x] create logical network ID/name.
- [x] persist membership.
- [x] select local NAP only if capability permits.
- [x] establish local subnet.

**Acceptance**

- `CreateNetwork` yields stable daemon state on a NAP-capable Linux node.
- Unsupported NAP capability produces a clear error rather than a model-name special case.
- Hardware acceptance is recorded in `docs/P6-001-HARDWARE-EVIDENCE-2026-09-01.md`; the production daemon created network `26ed3f29d622ae9c5c68635f4d548bbe`, persisted stable member state, created `brb-26ed3f29` with `10.201.41.1/24`, registered a live NAP, and accepted `arisu` so the server-side Bluetooth interface `enxf4d10870b786` became a kernel member of that exact bridge.

## P6-002 — Implement discoverable BlueRoute network identity

- [x] identify nearby BlueRoute-capable peers/networks.
- [x] do not rely on display/Bluetooth name for security.
- [x] document limitations.

**Acceptance**

- Second compatible Linux node discovers a candidate network without manual MAC entry.
- Hardware acceptance is recorded in `docs/P6-002-HARDWARE-EVIDENCE-2026-09-02.md`; `debiancb1` created network `4bc4e5b829838b47c985ec66881306fa` and `arisu` discovered that exact logical `NetworkId` through the production discovery probe without receiving a Bluetooth MAC address.

## P6-003 — Implement join approval/trust workflow

- [x] pair if needed.
- [x] BlueRoute membership approval.
- [x] persist accepted membership.
- [x] reject unauthorized nodes.

**Acceptance**

- Joining requires intended trust under initial security model.
- Hardware acceptance is recorded in `docs/P6-003-HARDWARE-EVIDENCE-2026-09-02.md`; the production daemon on `debiancb1` approved `arisu` node `4d5f049ad9b6ea51d782250478fa3ebf` for network `4bc4e5b829838b47c985ec66881306fa` as `member=0, trusted=1`, repeated approval without changing the durable file hash, and then revoked the peer while preserving the network record.

## P6-004 — Implement join-network operation

**Dependency:** The existing transactional coordinator, fail-closed state handling, rollback tests and authorization are implemented (see `docs/P6-004-JOIN-ORCHESTRATION.md`). The production `JoinNetwork` preflight *intentionally* returns `CapabilityUnavailable` until authenticated P7-001..P7-004 control sessions are available. Address assignment groundwork from P6-005 is complete but production join is not. Do not equate an acceptance-probe PANU connection with an authenticated member.

- [x] implement transaction/rollback and authorization orchestration groundwork.
- [ ] establish PANU link.
- [ ] obtain/assign IP.
- [ ] start control session.
- [ ] mark joined only after required networking succeeds.

**Acceptance**

- Compatible node joins without manual Bluetooth/network shell commands.

## P6-005 — Implement IPv4 allocation for one star

- [x] select private subnet policy.
- [x] detect route conflicts.
- [x] assign addresses.
- [x] avoid unnecessary durable transient state.

**Acceptance**

- Repeated create/join cycles do not accumulate conflicts.
- **COMPLETE (scope: IP allocation, not P6-004 authenticated join):** merged at `c2db78108ba27c82e961bd0d0092d6a0ad459b42`, exact-head CI passed, two live NetworkManager apply/remove cycles recorded in `docs/P6-005-HARDWARE-EVIDENCE-2026-09-02.md`. See `docs/P6-005-IPV4-ALLOCATION.md`.

## P6-006 — Prove ordinary application traffic

**Two distinct gates:** Existing [PR #41](https://github.com/ekkus93/blueroute/pull/41) adds standalone host/client acceptance programs, TCP/UDP helper and runbook, but remains unmerged. Review and merge software-qualified tooling (at exact head, preserving security) without asserting production JoinNetwork is functional or raw-IP hardware checks passed. Keep original physical criteria pending in the hardware backlog.

- [ ] qualify and reconcile/merge existing PR #41 software acceptance tooling into `master` (not a duplicate PR).
- [ ] ping.
- [ ] SSH.
- [ ] TCP bulk transfer.
- [ ] UDP test.
- [ ] separate raw IP success from name-resolution behavior.

**Acceptance**

- Applications require no BlueRoute-specific library.

## P6-007 — Add third and subsequent clients

- [ ] connect multiple PANU nodes up to conservative local capability/policy ceiling.
- [ ] concurrent traffic.
- [ ] expose capacity/capability diagnostics.

**Acceptance**

- Stable multi-client star works at documented count for tested adapter.
- Count is not treated as universal.

## P6-008 — Implement leave-network operation

- [ ] tear down runtime membership.
- [ ] remove BlueRoute-owned addresses/routes/profiles.
- [ ] retain trust only according to policy.

**Acceptance**

- Leave is idempotent and leaves no stale data-plane state.

## P6-009 — Implement daemon restart reconciliation

- [ ] restart with PAN links present.
- [ ] re-observe system state.
- [ ] reconcile instead of duplicating.

**Acceptance**

- Daemon restart requires no reboot/manual cleanup.

## P6-010 — Implement link-loss reconnect

- [ ] detect lost peer.
- [ ] degraded/reconnecting state.
- [ ] bounded backoff.
- [ ] restore health when peer returns.

**Acceptance**

- Range-loss/recovery works on physical test hardware without manual commands.

---

# P7 — Inter-node control plane

**Critical prerequisite for P6-004:** Implement P7-001..P7-004 ahead of completing production join. Authenticated control sessions must bind approved NodeId and the intended NetworkId; MAC address, source IP, BlueZ `Trusted`, advertised identity and unverified hello messages are not authority. Test replay/malformed/duplicate-session cases without hardware. Follow immediately with P7-005 parser hardening and early P14 threat modeling; defer physical session acceptance, not security checks.

## P7-001 — Choose control-plane transport

- [x] evaluate transport over established IP PAN.
- [x] port/address discovery.
- [x] authentication binding to BlueRoute identity.
- [x] ADR.

**Design decision:** See `docs/P7-001-CONTROL-TRANSPORT.md`; TCP-over-PAN with mutually authenticated TLS 1.3 and explicit per-node key pinning. Production authentication/enrollment is **not implemented** (P7-003/004); `JoinNetwork` remains fail-closed.

**Acceptance**

- Source IP is not sole identity proof.

## P7-002 — Define control protocol envelope

- [x] version.
- [x] sender/network identity.
- [x] message type.
- [x] bounded payload.
- [x] freshness/replay approach.

**Software complete:** `crates/blueroute-protocol/src/control.rs`, exact-head CI `38397384422`; parser and replay guard do not authenticate untrusted identity.

**Acceptance**

- Malformed/oversized/unknown-critical data is safely rejected.

## P7-003 — Implement peer hello/capability exchange

- [x] software/protocol version represented in envelope/hello.
- [x] stable identity claimed by envelope sender (not authenticated yet).
- [x] node capability facts are typed and validated.
- [x] canonical IPv4 segment and direct-neighbor facts are represented and bounded.
- [x] implement software key-pin matching against durable network/NodeId approval, with revocation and persistence tests.
- [x] validate bidirectional hello/ack transcript challenge echoes, immutable claimed capabilities, peer/network/session scope and replay (software-only, not TLS authentication; [CI 38455747473](https://github.com/ekkus93/blueroute/actions/runs/38455747473) at `dd51f9c`).
- [ ] implement user-verified key enrollment, TLS peer certificate proof, authenticated hello/ack exchange and channel binding (depends on P7-004).

**Qualified P7-003 groundwork:** `crates/blueroute-protocol/src/peer_pin.rs` requires matching approved network, logical NodeId, and verified TLS SPKI fingerprint. `apps/daemon/src/peer_pin_store.rs` persists key pins with `SecretFileStore` (owner-only directory/file, atomic update, fail-closed malformed/future/duplicate/symlink rejection). `apps/daemon/src/peer_trust.rs` checks persisted pins in addition to P6-003 trust, removes pins on revocation, and refuses trust-only runtime for crypto authorization. Exact-head [CI 38448995440](https://github.com/ekkus93/blueroute/actions/runs/38448995440) passed. **Not yet an actual TLS verified peer**: key generation, user-verified enrollment ceremony, rustls mutual certificate checking, session channel binding and live hello exchange remain unchecked. Old NodeId-only records do not automatically become key pins; hardware tests remain deferred.

**Additional P7-003 software qualification (2026-10-13):** Production daemon startup now loads the owner-only pin store, refusing corrupted/insecure records; exact-head [CI 38479338847](https://github.com/ekkus93/blueroute/actions/runs/38479338847) at `7bd47af5` passed. A same-network public-key alias across different logical NodeIds now fails closed both in memory and on disk, and a policy helper requires matching independently confirmed and already-TLS-verified key fingerprints plus durable NodeId approval before recording enrollment; exact-head [CI 38480341965](https://github.com/ekkus93/blueroute/actions/runs/38480341965) at `36596f2f` passed. Canonical lowercase 64-digit SHA-256 SPKI fingerprint parsing/formatting and persisted storage now share a single strict implementation ([CI 38481217364](https://github.com/ekkus93/blueroute/actions/runs/38481217364) at `76cddd52`). **Neither a real mutual-TLS verifier nor user confirmation UI/service exists yet.** Do not pass unverified claimed fingerprints into this helper or enable production join; the checked software subtasks and deferred physical tests remain separately tracked.

**Acceptance**

- Peers negotiate capability without knowing each other's computer model.

## P7-004 — Implement control-session lifecycle

- [ ] establish authenticated TLS after data-plane availability.
- [x] model logical session loss and stale event isolation (software model).
- [x] model reconnect/replacement and session capacity (software model).
- [x] reject duplicate concurrently active logical-peer sessions (software model).
- [x] reject reused session IDs across retained peers and after loss/reconnect; enforce ordered TCP/TLS/hello/active stages and deterministic 5s/10s/30s deadline budgets (software-only; [CI 38458952233](https://github.com/ekkus93/blueroute/actions/runs/38458952233) at `f4bb6906`).
- [ ] implement real TLS peer-key binding, runtime transport and bounded loss/reconnect cleanup.

**Not fully accepted:** `crates/blueroute-protocol/src/session.rs` includes a bounded retry budget, deterministic 250ms–30s exponential delay, stale-session isolation, retained-peer nonce uniqueness and ordered timeout budgets. Exact-head [CI 38458952233](https://github.com/ekkus93/blueroute/actions/runs/38458952233) passed at `f4bb6906`. `control_io.rs` now also has absolute whole-frame TCP read/write deadlines and slow-peer timeout tests ([CI 38495495929](https://github.com/ekkus93/blueroute/actions/runs/38495495929) at `fa6d9590`), but **not authenticated TLS**; duplicate loss/reconnect transitions now fail closed ([CI 38481440066](https://github.com/ekkus93/blueroute/actions/runs/38481440066) at `dd5c71cf`); the deadline model still requires cancellable real I/O integration, verified certificate/channel binding, and production join remains fail-closed.

**Acceptance**

- Repeated PAN reconnects do not leak sessions/tasks.

## P7-005 — Fuzz/property-test parser

- [x] deterministic malformed/truncated/oversized/unknown-field input tests for control parser.
- [x] per-message round-trip and frame-mutation tests.
- [x] control-version rejection/compatibility tests.
- [ ] connect parser to authenticated daemon transport and show peer data cannot crash daemon; full acceptance remains pending P7-004.

**Parser software tests qualified** at `964b7add` (exact-head CI `38399834487` passed), with strict nested Hello variant field rejection added at `51960025` ([CI 38480899801](https://github.com/ekkus93/blueroute/actions/runs/38480899801)); incremental EOF/truncation detection added at `0322101b` ([CI 38490454756](https://github.com/ekkus93/blueroute/actions/runs/38490454756)); deterministic checks exercise the shared protocol parser but not an authenticated daemon session or physical Bluetooth device.

**Acceptance**

- Tested invalid peer data cannot panic daemon.

---

# P8 — Routed interconnected-star networking

**Software-first:** Use deterministic topology graphs, simulated peers and Linux namespaces/veth with kernel IP forwarding/routing where feasible; exercise loss/convergence/cleanup. Physical two-hop and interconnected-star Bluetooth traffic under P8-006/P8-007 stays **HARDWARE DEFERRED**, not completed by virtual networking.

## P8-001 — Build neighbor observation model

- [x] direct PAN links derived from verified active topology graph, not peer advertisements.
- [x] candidate discovery observations represented separately from active links.
- [x] locally measured monotonic timestamps, 120s expiry and older-observation suppression.
- [x] bounded per-candidate capability annotations (256 candidates; untrusted until verified).

**Software model complete:** `crates/blueroute-core/src/neighbor.rs` separates discovery and active PAN states; exact-head [CI 38469158021](https://github.com/ekkus93/blueroute/actions/runs/38469158021) passed at `56208e3`. This module does not authenticate peers, ingest production BlueZ events, or establish physical routing; actual integration belongs to P7/P8-005 and radio acceptance remains deferred.

**Acceptance**

- Physical/candidate graph is distinct from active routed graph.

## P8-002 — Implement distinct subnet allocation per PAN segment

- [x] deterministic non-overlapping prefix reservations per SegmentId, preserving P6-005 bootstrap subnet.
- [x] detect configured RFC1918 pool and local active host-route/address conflicts; bound candidate probing.
- [x] idempotent owner-scoped *reservation* reclamation with independent segment allocations preserved.
- [ ] wire reservations to the privileged Linux backend and prove ownership-safe address/route teardown across restart/reconciliation.

**Software model qualified:** `crates/blueroute-core/src/segment_allocator.rs` with exact-head [CI 38470193934](https://github.com/ekkus93/blueroute/actions/runs/38470193934) at `2c2c108`. A subsequent regression test at `ed628447` ([CI 38477587852](https://github.com/ekkus93/blueroute/actions/runs/38477587852)) ensures an existing reservation fails closed if new foreign host routes overlap it, without mutating the owned lease. The allocator does not install or remove kernel routes; real multi-hop Bluetooth acceptance under P8-006..007 stays **HARDWARE DEFERRED**.

**Acceptance**

- Multiple PAN stars coexist without address overlap.

## P8-003 — Implement forwarding on routing nodes

- [x] compute desired IPv4 forwarding only for explicitly routing-capable nodes with live links on at least two distinct PAN segments (pure software policy).
- [ ] apply/release forwarding through the existing privileged Linux ownership lease only when desired state changes.
- [ ] preserve host security policy and unrelated forwarding ownership during reconciliation.
- [ ] cleanup when unnecessary; qualify simulated topology loss/recovery and Linux namespaces where available.

**Software model qualified:** `crates/blueroute-core/src/forwarding_policy.rs` passed [exact-head CI 38478135480](https://github.com/ekkus93/blueroute/actions/runs/38478135480) at `167221e`. Unknown routing capability denies forwarding, a single L2 PAN star does not request global IPv4 forwarding, and disconnected/failed links cannot sustain forwarding intent. This is not a kernel/sysctl integration or real Bluetooth acceptance.

**Acceptance**

- Routing node forwards BlueRoute traffic between test PAN segments.

## P8-004 — Implement route computation v1

- [x] compute paths over observed active PAN link graph with nonzero link metrics.
- [x] deterministic desired node-route set owned by the explicit BlueRoute NetworkId.
- [x] stable-path preference using degraded-link penalty, hop-count and first-hop tie-break.
- [x] unreachable/failed/disconnected detection without inventing default Internet routes.
- [x] require affirmative routing capability on transit nodes (unknown is not approval).

**Software route planner qualified:** `crates/blueroute-core/src/route_planner.rs`, exact-head [CI 38471055209](https://github.com/ekkus93/blueroute/actions/runs/38471055209) passed at `32d4954`. This module does **not** authenticate remote topology claims, distribute route updates, modify Linux routes or prove Bluetooth traffic. P7 and P8-005/006/007 remain separate gates.

**Acceptance**

- Unit tests cover line/tree/redundant/partitioned and heterogeneous-capability graphs.

## P8-005 — Implement route distribution/application

- [ ] distribute desired routes.
- [ ] apply through Linux backend.
- [ ] remove stale routes.
- [ ] reject foreign/non-member routes.

**Acceptance**

- Route table matches expected topology after changes.

## P8-006 — Prove two-hop TCP/IP

```text
client A -> router B -> client C
```

- [ ] ping.
- [ ] TCP.
- [ ] UDP.
- [ ] record performance vs direct link.

**Acceptance**

- A and C exchange normal IP traffic without a direct PAN link.

## P8-007 — Prove interconnected stars

```text
A/B clients -> hub 1 -> hub 2 -> C/D clients
```

- [ ] two PAN segments.
- [ ] route across hubs.
- [ ] bidirectional traffic.

**Acceptance**

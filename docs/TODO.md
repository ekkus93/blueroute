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

**Status date:** 2026-10-14 (P7 post-hello session-stream scope/replay and abrupt-EOF tests qualified through `5c57bc06`, [CI 38604630252](https://github.com/ekkus93/blueroute/actions/runs/38604630252) passed; PR #41 remains open with Ralph merge tooling returning `INVALID_ARGUMENT`. Real mutual TLS, independently approved key enrollment, and production join are **NOT** complete.)

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
| P7-003 | `[-]` | **IN PROGRESS**: typed validated hello/capability/topology claims plus fail-closed `(NetworkId,NodeId,SPKI-SHA256)` key pinning, secure owner-only durable enrollment records and daemon approval checks. Peer revocation erases persisted pin; reapproval does not restore it. [CI 38448995440](https://github.com/ekkus93/blueroute/actions/runs/38448995440) passed at `897d8cdb`. Daemon startup validates and wires owner-only peer-key pins ([CI 38479338847](https://github.com/ekkus93/blueroute/actions/runs/38479338847)); duplicate key-to-NodeId aliases are rejected and independent fingerprint confirmation is required by the durable enrollment policy helper ([CI 38480341965](https://github.com/ekkus93/blueroute/actions/runs/38480341965) at `36596f2f`). Strict canonical SHA-256 fingerprint parsing/display is shared between protocol and persistence ([CI 38481217364](https://github.com/ekkus93/blueroute/actions/runs/38481217364) at `76cddd52`). Real TLS identity verification, authenticated hello exchange and key enrollment UX still missing. Canonical NetworkId/NodeId TLS exporter context and exporter-session-ID transcript checks passed [CI 38496556498](https://github.com/ekkus93/blueroute/actions/runs/38496556498) at `37e56d3c`; these are binding-policy groundwork, NOT actual TLS peer verification. |
| P7-004 | `[-]` | **IN PROGRESS**: bounded peer registry/stale-event isolation and bounded exponential reconnect policy passed exact-head [CI 38444858686](https://github.com/ekkus93/blueroute/actions/runs/38444858686) at `c688f799`. Duplicate loss notifications and repeated reconnect-state wakeups are now idempotent ([CI 38481440066](https://github.com/ekkus93/blueroute/actions/runs/38481440066) at `dd5c71cf`). Raw TCP frame I/O now has absolute whole-frame read/write deadlines, timeout restoration, and slow-peer/expired-deadline tests ([CI 38495495929](https://github.com/ekkus93/blueroute/actions/runs/38495495929) at `fa6d9590`). This is transport groundwork, NOT authenticated TLS. Key-pinned peer verification, TLS I/O, actual reconnection transport execution, and daemon integration remain unimplemented. A bounded per-peer reconnect attempt scheduler now deduplicates loss/wakeups, enforces retry exhaustion and stale-generation isolation ([CI 38496100665](https://github.com/ekkus93/blueroute/actions/runs/38496100665) at `52b873e7`). Absolute-deadline framed I/O now rejects even a final read/write completion delivered after the budget expires; deterministic late-adapter tests passed [CI 38552983944](https://github.com/ekkus93/blueroute/actions/runs/38552983944) at `6ef8ab84`. A bounded symmetric Hello/HelloAck exchange now uses deadline-aware transport I/O and rejects mismatched peer/network/session, challenge reflection, invalid acknowledgements, truncated streams and expired deadlines; [exact-head CI 38552212141](https://github.com/ekkus93/blueroute/actions/runs/38552212141) passed at `eaf5c0a2`. This is a **claims-only** exchange and not mutual TLS or production authentication. Two simulated peers now complete the same exchange over localhost TCP; independent network, NodeId, exporter and reflected-challenge failures are rejected. [CI 38552630252](https://github.com/ekkus93/blueroute/actions/runs/38552630252) passed at `0ee2accb` (simulation only). |

**P7-004 outbound post-hello stream (software-only, 2026-10-14):** `crates/blueroute-protocol/src/session_writer.rs` pairs a locally scoped Heartbeat/Goodbye encoder with the existing inbound replay/Goodbye parser: monotonic outbound sequence allocation after Hello/HelloAck, exporter-session and local-node envelope scope, invalid/self-peer and zero-session rejection, terminal Goodbye, and permanent sequence-overflow rejection. Deterministic peer round-trip, fragmentation, wrong-peer/exporter, and exhaustion tests passed [exact-head CI 38672265801](https://github.com/ekkus93/blueroute/actions/runs/38672265801) at `a8a30663e333955a34d2d3f011355088ff46e130`. **No mutual TLS verification, key approval UI, authenticated daemon channel, real reconnect transport, PAN traffic, or production JoinNetwork is established by these tests.**


**P7-005 hostile coalesced-batch atomicity (software only, 2026-10-14):** Five deterministic regressions establish that a valid heartbeat followed by a wrong-peer frame or replay in the *same read batch* cannot refresh the idle deadline, a Goodbye with a trailing partial header cannot become graceful, a 65-frame flood is rejected atomically, and two valid coalesced heartbeats are accepted. Exact-head [CI 38681277277](https://github.com/ekkus93/blueroute/actions/runs/38681277277) passed at `5d07bd8f3ccee0988e789f8ed6537e14dbbc3227`. This is **not** mutual TLS, authenticated key enrollment, daemon integration, physical PAN or production join evidence; P7-003/P7-004/P6-004 remain open.

| P7-005 | `[-]` | **IN PROGRESS**: parser fuzz-style deterministic mutation, frame truncation, version incompatibility and round-trip tests passed exact-head [CI 38399834487](https://github.com/ekkus93/blueroute/actions/runs/38399834487) at `964b7add`. Unknown fields within Hello message variants fail strict parsing ([CI 38480899801](https://github.com/ekkus93/blueroute/actions/runs/38480899801) at `51960025`). Strict nested capability claims and invalid zero-capacity/battery claims also fail closed ([CI 38484623688](https://github.com/ekkus93/blueroute/actions/runs/38484623688) at `06e5b38a`). Incremental frame decoder rejects EOF on any partial header/body; exact-head [CI 38490454756](https://github.com/ekkus93/blueroute/actions/runs/38490454756) passed at `0322101b`. Daemon wire integration/fuzz acceptance still depends on P7-004. |
| P10-001 | `[-]` | **IN PROGRESS:** early read-only CLI uses the version-gated `blueroute-client` and has `--help`/`--version`, preliminary usage and daemon error exits; **clap**, complete exit taxonomy and further commands remain. Exact-head [CI 38461039721](https://github.com/ekkus93/blueroute/actions/runs/38461039721) passed at `51160db`. |
| P10-002 | `[-]` | **IN PROGRESS:** `status`, `capability show`, and `network list` have human/JSON output. `node list` and `node show <node-id>` parse with strict NodeId validation and typed version-gated daemon requests. A conservative daemon local-only NodeId summary is implemented; unknown remote IDs produce no record, never invented reachability. Real D-Bus broker and restart tests passed exact-head [CI 38607313157](https://github.com/ekkus93/blueroute/actions/runs/38607313157) at `0743af18`. Authenticated remote-node inventory, actual display names and live reachability remain pending P7/runtime integration; this is **not** end-to-end peer discovery. |
| P14-001 | `[x]` | **COMPLETE (threat model only):** `docs/P14-001-THREAT-MODEL.md`, [CI 38466101723](https://github.com/ekkus93/blueroute/actions/runs/38466101723); implementation safeguards and physical acceptance remain separate. |
| P8-001 | `[x]` | **SOFTWARE COMPLETE (observation model):** `NeighborObservations` keeps expiring untrusted discovered candidate peers separate from active observed `TopologyGraph` PAN links, with bounded capacity, monotonic timestamp ordering and capability annotations; [CI 38469158021](https://github.com/ekkus93/blueroute/actions/runs/38469158021) passed at `56208e3`. Live BlueZ/P7 ingestion remains P8 integration, not evidence of RF behavior. |
| P8-002 | `[-]` | **IN PROGRESS:** deterministic non-overlapping multi-segment private IPv4 allocation, host-prefix collision checks and owner-scoped *reservation* release passed [CI 38470193934](https://github.com/ekkus93/blueroute/actions/runs/38470193934) at `2c2c108`. A regression fix now revalidates newly observed foreign routes before reusing an existing reservation; exact-head CI 38477587852 passed at `ed628447`. Live kernel route/address ownership teardown and actual PAN segment integration remain; no physical hardware claim. |
| P8-003 | `[-]` | **IN PROGRESS:** deterministic fail-closed multi-PAN forwarding intent requires explicitly routing-capable local node and active links in at least two distinct PAN segments; [CI 38478135480](https://github.com/ekkus93/blueroute/actions/runs/38478135480) passed at `167221e`. Actual privileged forwarding reconciliation, owner-scoped lease and kernel network namespace qualification remain. |
| P8-004 | `[x]` | **SOFTWARE COMPLETE (desired-route calculation):** deterministic weighted node routes over active PAN graph respect explicitly routing-capable transit nodes, stable healthy paths and partitions; [CI 38471055209](https://github.com/ekkus93/blueroute/actions/runs/38471055209) passed at `32d4954`. Applying/distributing privileged routes remains P8-005 and physical multi-hop tests remain deferred. |
| P14-004 | `[-]` | **IN PROGRESS:** shared `DisplayName` rejects control/escape and Unicode bidirectional-override characters while preserving printable international names; unit tests pass [CI 38467939586](https://github.com/ekkus93/blueroute/actions/runs/38467939586) at `939935e`. Full peer-field limits, unsupported messages and fuzz/domain conversion review still outstanding. |
| P10-003 | `[-]` | **IN PROGRESS:** network creation and discovery start/stop delegate to PolicyKit-protected typed daemon API; parsing/validation tests pass [CI 38466994092](https://github.com/ekkus93/blueroute/actions/runs/38466994092) at `c786f77`. Production join/leave and full end-to-end lifecycle remain. |
| P10-004 | `[-]` | **IN PROGRESS:** explicit full-NodeId `peer trust`/`peer forget` invoke existing PolicyKit-protected D-Bus commands; tests and [CI 38466585585](https://github.com/ekkus93/blueroute/actions/runs/38466585585) at `8183225` passed. Pending requests/authenticated-join UX remain; trust is **not** TLS enrollment or membership. |
| All other tasks | `[ ]` | Not started unless explicitly documented in that task; remaining hardware acceptance is not a blocker for independent software work. |

**P7-003/P7-004 durable-pin-gated inbound control (software only, 2026-10-14):** `apps/daemon/src/pinned_ingress.rs` ties a previously authenticated post-Hello parser to a fresh on-disk network-approval and enrolled SPKI-pin check for every frame/EOF operation. The in-process trust mutex now encloses both authorization and parsing, preventing local revocation from interleaving between their checks. Unknown or wrong keys, trust-only approval, revocation/reapproval and forged/deadline-expired frames remain fail-closed and permanently terminate the modeled session. [Exact-head CI 38683882275](https://github.com/ekkus93/blueroute/actions/runs/38683882275) passed at `dc6721946a1553e37515f9f78d5ce5827a38132a`. **This is not real mutual TLS, independently approved enrollment UX, external-process locking, active daemon ingress, authenticated JoinNetwork, or physical PAN acceptance.** Those prerequisites remain open.

**P7-003/P7-004 durable-state corruption and simulated multi-peer isolation (software-only, 2026-10-14):** Two additional integration regressions show that a corrupt pin file permanently poisons the pending session, even after the on-disk state is repaired, and that separate enrolled peers cannot substitute their `NodeId`, SPKI fingerprint, or frames for another peer's session. [Exact-head CI 38684585143](https://github.com/ekkus93/blueroute/actions/runs/38684585143) passed at `c7152e70222a49455f7838ba4fdb4325eb9b22b4`. These simulate already-verified TLS context; **real mutual TLS, independent enrollment UX, cross-process locking, production daemon sockets and Bluetooth PAN remain unimplemented/unqualified**.

**P7-003/P7-004 type-level ingress precondition (software-only, 2026-10-14):** The daemon's `PinnedSessionIngress::after_verified_handshake` no longer accepts arbitrary caller-supplied `NetworkId`, `NodeId`, key fingerprint and exporter bytes as separate public arguments. It now consumes an opaque `VerifiedTlsIdentity` with private fields and **no production constructor**; only synthetic module-internal unit tests can create it until actual TLS certificate/key verification and exporter derivation are implemented. [Exact-head CI 38685477595](https://github.com/ekkus93/blueroute/actions/runs/38685477595) passed at `dceec21456052830b441b9e4efe83ff24d18ce13`. This is a fail-closed API barrier, not a real TLS handshake, channel binding, enrollment ceremony or production join. Those implementation and physical gates remain open.

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

**Additional P7-003 software qualification (2026-10-13):** Production daemon startup now loads the owner-only pin store, refusing corrupted/insecure records; exact-head [CI 38479338847](https://github.com/ekkus93/blueroute/actions/runs/38479338847) at `7bd47af5` passed. A same-network public-key alias across different logical NodeIds now fails closed both in memory and on disk, and a policy helper requires matching independently confirmed and already-TLS-verified key fingerprints plus durable NodeId approval before recording enrollment; exact-head [CI 38480341965](https://github.com/ekkus93/blueroute/actions/runs/38480341965) at `36596f2f` passed. Canonical lowercase 64-digit SHA-256 SPKI fingerprint parsing/formatting and persisted storage now share a single strict implementation ([CI 38481217364](https://github.com/ekkus93/blueroute/actions/runs/38481217364) at `76cddd52`). **Neither a real mutual-TLS verifier nor user confirmation UI/service exists yet.** Exporter context, label and exact exporter-session-ID transcript checks have negative tests ([CI 38496556498](https://github.com/ekkus93/blueroute/actions/runs/38496556498) at `37e56d3c`). The future TLS stack must derive the exporter output from a fully authenticated connection; this helper does not establish proof or enroll keys. Do not pass unverified claimed fingerprints into this helper or enable production join; the checked software subtasks and deferred physical tests remain separately tracked.

**P7-003 durable peer-key write serialization (software only, 2026-10-14):** `DurablePeerTrustOperations` now serializes the daemon's membership approval, key enrollment, pin revocation, and pin-authorization read/modify/write paths behind an in-process fail-closed mutex, preventing simultaneous daemon requests from overwriting peer records or resurrecting keys on overlapping revocation. A 12-worker synchronized enrollment test verifies that all distinct approved NodeIds and their separately verified key pins survive durable persistence; revocation followed by NodeId reapproval still does not restore a pin. [Exact-head CI 38675135882](https://github.com/ekkus93/blueroute/actions/runs/38675135882) passed at `b0c62ed3403a3a9e488fc6ee64e14bdd37a51998`. **The lock coordinates only this one daemon instance**, not external file writers or additional processes; verified TLS key proof, user enrollment ceremony, live revocation of active TLS channels, and production JoinNetwork remain unimplemented. Test digests are synthetic and are not TLS/Bluetooth acceptance evidence.

**Acceptance**

- Peers negotiate capability without knowing each other's computer model.

## P7-004 — Implement control-session lifecycle

**Additional P7-004 EOF fault-injection qualification (2026-10-14):** `SessionControlStream::finish()` now treats graceful Goodbye+EOF as terminal, rejects abrupt EOF without Goodbye and refuses any transport reuse; deterministic tests pass exact-head [CI 38604630252](https://github.com/ekkus93/blueroute/actions/runs/38604630252) at `5c57bc069ef92e6bf559727ba950ebae71df3bdb`. This is parser lifecycle hardening, **not** authenticated TLS or physical network evidence.

**Additional software-only P7-004 hardening (2026-10-14):** `crates/blueroute-protocol/src/session_stream.rs` bounds decoded post-hello frames to one expected (NetworkId, NodeId, TLS exporter session ID), rejects sequence replay/staleness after handshake messages 1/2, rejects extra Hello/HelloAck and post-Goodbye traffic, and poisons the entire session on malformed or mismatched frames. A partial batch is never returned after a later invalid frame. Exact-head [CI 38601668411](https://github.com/ekkus93/blueroute/actions/runs/38601668411) passed on `9878dca724f4d208ed007093869fd21a57a9ea8b`. **Critical:** this is parser/scope groundwork, not authentication: no real mutual TLS, cryptographic exporter derivation, approval UX, daemon socket integration or production JoinNetwork was added. Hardware acceptance remains deferred.

- [ ] establish authenticated TLS after data-plane availability.
- [x] model logical session loss and stale event isolation (software model).
- [x] model reconnect/replacement and session capacity (software model).
- [x] reject duplicate concurrently active logical-peer sessions (software model).
- [x] reject reused session IDs across retained peers and after loss/reconnect; enforce ordered TCP/TLS/hello/active stages and deterministic 5s/10s/30s deadline budgets (software-only; [CI 38458952233](https://github.com/ekkus93/blueroute/actions/runs/38458952233) at `f4bb6906`).
- [ ] implement real TLS peer-key binding, runtime transport and bounded loss/reconnect cleanup.

**Not fully accepted:** `crates/blueroute-protocol/src/session.rs` includes a bounded retry budget, deterministic 250ms–30s exponential delay, stale-session isolation, retained-peer nonce uniqueness and ordered timeout budgets. Exact-head [CI 38458952233](https://github.com/ekkus93/blueroute/actions/runs/38458952233) passed at `f4bb6906`. `control_io.rs` now also has absolute whole-frame TCP read/write deadlines and slow-peer timeout tests ([CI 38495495929](https://github.com/ekkus93/blueroute/actions/runs/38495495929) at `fa6d9590`), but **not authenticated TLS**; duplicate loss/reconnect transitions now fail closed ([CI 38481440066](https://github.com/ekkus93/blueroute/actions/runs/38481440066) at `dd5c71cf`); the deadline model still requires cancellable real TLS I/O integration, verified certificate/channel binding, and production join remains fail-closed. A deterministic bounded per-peer retry scheduler with one-attempt wakeups, independent peer budgets, monotonic time checks, stale-generation cleanup and exhaustion tests is now qualified ([CI 38496100665](https://github.com/ekkus93/blueroute/actions/runs/38496100665) at `52b873e7`); it does not open sockets or authorize peers.

**P7-004 outbound post-hello stream (software-only, 2026-10-14):** `crates/blueroute-protocol/src/session_writer.rs` pairs a locally scoped Heartbeat/Goodbye encoder with the existing inbound replay/Goodbye parser: monotonic outbound sequence allocation after Hello/HelloAck, exporter-session and local-node envelope scope, invalid/self-peer and zero-session rejection, terminal Goodbye, and permanent sequence-overflow rejection. Deterministic peer round-trip, fragmentation, wrong-peer/exporter, and exhaustion tests passed [exact-head CI 38672265801](https://github.com/ekkus93/blueroute/actions/runs/38672265801) at `a8a30663e333955a34d2d3f011355088ff46e130`. **No mutual TLS verification, key approval UI, authenticated daemon channel, real reconnect transport, PAN traffic, or production JoinNetwork is established by these tests.**

**Further P7-004 bounded post-hello I/O (software only, 2026-10-14):** The outbound `SessionControlWriter` now has absolute-deadline Heartbeat/Goodbye send methods that poison the entire logical session after timeout, interrupted/partial failure or late transport completion. The inbound `SessionControlStream::read_one_with_deadline` validates each fully read frame's NodeId, NetworkId, exporter-session ID and monotonic sequence, and irreversibly poisons on truncated/expired/malformed read or wrong context. Deterministic deadline-memory adapter, partial-write, truncated header/body, wrong-context, and terminal Goodbye tests passed [exact-head CI 38674558022](https://github.com/ekkus93/blueroute/actions/runs/38674558022) at `3f065deac13147199f8051d3417db32b6fccec06`. Both operate on **caller-supplied simulated verified sessions only**; they do NOT verify mutual TLS, perform independent key enrollment, establish runtime reconnect or authenticate BlueZ traffic. **P7-003/P7-004 and production P6-004 remain open.**


**P7-004 authenticated-inbound activity deadline coupling (software-only, 2026-10-14):** `crates/blueroute-protocol/src/session_activity.rs` now connects a previously validated post-Hello frame parser to its active-stage monotonic idle clock: incomplete frames, empty polls and malformed/wrong-peer control messages cannot renew liveness, while deadline expiry, clock regression, invalid scope and unexpected EOF terminate the logical session. Goodbye moves into bounded closing, with terminal EOF handling. Deterministic simulated-peer/unit regression coverage passed [exact-head CI 38678299553](https://github.com/ekkus93/blueroute/actions/runs/38678299553) at `71a8fc8b117a320ce7a956198d4aaad8254c466d`. This helper assumes an independently authenticated channel; **no mutual TLS, user-approved enrollment, real daemon socket, physical Bluetooth or production JoinNetwork is implemented by it**. P7-003/P7-004 security gates and hardware acceptance remain open.

**P7-004 bounded Goodbye/EOF (software-only, 2026-10-14):** `AuthenticatedInboundActivity::finish_at` now refuses a late EOF after a verified Goodbye, and any unexpected plaintext after Goodbye irreversibly terminates the logical session. Separate regression cases distinguish clean Goodbye+EOF, early EOF, timeout expiry, and post-Goodbye violations; exact-head [CI 38679210137](https://github.com/ekkus93/blueroute/actions/runs/38679210137) passed at `bda3176aa6b25e3a479283c03ae290576b3347b7`. This is bounded simulated transport logic; **actual mutual TLS, independent key enrollment, daemon integration, authenticated join and physical Bluetooth validation are not complete**.


**Acceptance**

- Repeated PAN reconnects do not leak sessions/tasks.

## P7-005 — Fuzz/property-test parser

**Additional P7-005 session-idle fault injection (software-only, 2026-10-14):** `crates/blueroute-protocol/tests/session_activity_faults.rs` covers every possible single split point of a bounded Heartbeat at the absolute idle deadline, two separately scoped peer sessions, replay poisoning, abrupt EOF and 2,048 reproducible hostile frame mutations. Exact-head [CI 38680491093](https://github.com/ekkus93/blueroute/actions/runs/38680491093) passed at `b377d98dce55551a7f940c4fa3ae9fe6c80440db`. All tests use synthetic already-verified channel assumptions; they **do not** authenticate peers, exercise daemon TLS, or prove any real Bluetooth behavior. Keep P7-005 daemon integration and physical acceptance open.

- [x] deterministic malformed/truncated/oversized/unknown-field input tests for control parser.
- [x] per-message round-trip and frame-mutation tests.
- [x] control-version rejection/compatibility tests.
- [ ] connect parser to authenticated daemon transport and show peer data cannot crash daemon; full acceptance remains pending P7-004.

**Additional P7-005 deterministic session-stream qualification (2026-10-14):** `crates/blueroute-protocol/tests/control_session_fuzz.rs` tests every selected fragmentation width across ordered Heartbeat/Goodbye transcripts and 2,048 reproducible hostile wire mutations, asserting no panics and irrecoverable poisoning on frame rejection. These tests run only against a simulated already-verified context (no TLS verification, enrollment, daemon transport, or Bluetooth hardware). Exact-head [CI 38649890223](https://github.com/ekkus93/blueroute/actions/runs/38649890223) passed at `1f92c50e6c5708f5a55b00d3c641b0856287aaab`. Authenticated daemon parser integration and malicious peer crash acceptance remain open until P7-003/P7-004 are implemented.

**Parser software tests qualified** at `964b7add` (exact-head CI `38399834487` passed), with strict nested Hello variant field rejection added at `51960025` ([CI 38480899801](https://github.com/ekkus93/blueroute/actions/runs/38480899801)); incremental EOF/truncation detection added at `0322101b` ([CI 38490454756](https://github.com/ekkus93/blueroute/actions/runs/38490454756)); clean EOF now terminally closes the decoder, rejecting subsequent bytes or parser reuse at `0743af18` ([CI 38607313157](https://github.com/ekkus93/blueroute/actions/runs/38607313157) passed); deterministic checks exercise the shared protocol parser but not an authenticated daemon session or physical Bluetooth device.

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

- Cross-star traffic works without giant layer-2 bridging.

## P8-008 — Implement route recovery after node loss

- [ ] remove failed next hop.
- [ ] alternate route when available.
- [ ] unreachable state otherwise.
- [ ] restore on return.

**Acceptance**

- Redundant physical topology survives router removal where alternate links exist.

## P8-009 — Evaluate dynamic routing protocol option

- [ ] compare BlueRoute orchestration with Babel or other mature option.
- [ ] evaluate convergence, security, observability, complexity, portability.
- [ ] ADR adopt/reject.

**Acceptance**

- Decision is evidence-based.

---

# P9 — Automatic topology management

**Software-first:** Test capability-aware placement, minimal plan changes, safe rollback, anti-flapping and simulated hub loss. Keep real Bluetooth hub-loss reformation (P9-005) **HARDWARE DEFERRED**.

## P9-001 — Define topology policy inputs

- [ ] connection capacity/capabilities.
- [ ] direct-neighbor availability.
- [ ] link quality/stability where portable.
- [ ] hop/path cost.
- [ ] topology stability.
- [ ] battery/power placeholder.
- [ ] future gateway preference.

**Acceptance**

- Inputs are explicit/testable and do not include hard-coded product models.

## P9-002 — Implement NAP role selection v1

- [ ] choose capable hub nodes automatically.
- [ ] respect per-node capacity.
- [ ] avoid unnecessary hubs.
- [ ] permit client-only nodes.

**Acceptance**

- Deterministic tests produce expected assignments across heterogeneous capabilities.

## P9-003 — Implement topology plan diff

- [ ] desired vs active topology.
- [ ] minimal ordered changes.
- [ ] keep working path until replacement exists where possible.

**Acceptance**

- Small metric changes do not rebuild whole network.

## P9-004 — Implement topology executor

- [ ] create links.
- [ ] remove obsolete links.
- [ ] coordinate routing.
- [ ] reconcile partial failures.

**Acceptance**

- Injected failure leaves daemon able to recover to valid state.

## P9-005 — Implement hub-loss reformation

- [ ] detect loss.
- [ ] select capable replacement/path.
- [ ] reconnect.
- [ ] recompute routes.

**Acceptance**

- Physical test recovers where radio/capability graph permits.

## P9-006 — Add topology anti-flap policy

- [ ] hysteresis/stability thresholds.
- [ ] failure backoff.
- [ ] prefer known-good links.

**Acceptance**

- Noisy metrics do not trigger continuous rebuild.

---

# P10 — CLI

**Execution priority raised:** Implement status/diagnostics/trust CLI soon after core P6/P7 dependencies, before multi-hop and GUI work. Read-only commands may start even earlier. Use `blueroute-client`/D-Bus; no direct privileged networking logic in the CLI.

## P10-001 — Create CLI skeleton with clap

- [ ] complete clap-based global options/parser (temporary minimal parser currently supports `--json`, `--help`).


- [x] daemon handling via version-gated shared system D-Bus client for read-only commands.
- [x] version display.
- [ ] consistent full error/exit taxonomy (currently preliminary usage, daemon-unavailable, operational errors).

**Software partial:** Initial read-only CLI in `apps/cli/src/main.rs` passed exact-head [CI 38461039721](https://github.com/ekkus93/blueroute/actions/runs/38461039721) at `51160db`; keep P10-001 open until clap integration and full exit-code contract.

**Acceptance**

- CLI contains no direct BlueZ/NetworkManager networking code.

## P10-002 — Implement status commands

- [x] `blueroute status` (typed D-Bus status; human/JSON).
- [x] `blueroute capability show` (typed capability summary; human/JSON).
- [-] `blueroute node list/show` (CLI human/JSON and daemon D-Bus local-node-only behavior qualified at `0743af18` including real-broker and daemon-restart tests; authenticated remote peers, names and reachability not integrated).
- [x] `blueroute network list` (read-only typed D-Bus, human/JSON; software-qualified under CI #38463280247 on `1471551`).

**Software partial:** stable JSON serialization round-trips and no direct BlueZ/NetworkManager access; exact-head [CI 38461039721](https://github.com/ekkus93/blueroute/actions/runs/38461039721) passed.

**Acceptance**

- Human output is useful and JSON is stable/tested.

**Additional P10-002 software evidence (2026-10-14):** CLI node list/show read-only commands, JSON serialization and negative full-NodeId parsing tests passed [CI 38603399314](https://github.com/ekkus93/blueroute/actions/runs/38603399314) at `bc11bacb`. Daemon `ListNodes`/`GetNode` now return only its own explicit local NodeId and `Local` reachability; unknown remote NodeIds are never synthesized. Real isolated D-Bus broker tests and shared-client queries before/after a daemon restart passed exact-head [CI 38607313157](https://github.com/ekkus93/blueroute/actions/runs/38607313157) at `0743af18`. Actual remote peer inventory, verified live reachability and device display names are **not** available and depend on P7/runtime integration. No hardware evidence is claimed.

## P10-003 — Implement network lifecycle commands

- [x] create (`blueroute network create <name>`; typed daemon/PolicyKit command, validated display name).
- [ ] join (production `JoinNetwork` must remain fail-closed until P7 TLS authentication).
- [ ] leave (daemon leave operation currently pending).
- [x] discover start/stop (typed daemon/PolicyKit commands).

**Software partial:** argument validation and delegation pass exact-head [CI 38466994092](https://github.com/ekkus93/blueroute/actions/runs/38466994092) at `c786f77`; this is not physical PAN or authenticated join acceptance.

**Acceptance**

- Single-star network can be managed end-to-end through daemon.

## P10-004 — Implement trust commands

- [ ] pending requests where applicable (key enrollment/approval UX still incomplete).
- [x] approve/trust (`blueroute peer trust <node-id>`, explicit full identifier; PolicyKit-gated by daemon).
- [x] forget/revoke (`blueroute peer forget <node-id>`, PolicyKit-gated; daemon revokes durable cryptographic pin if configured).

**Software partial:** [CI 38466585585](https://github.com/ekkus93/blueroute/actions/runs/38466585585) passed at `8183225`. These commands manage explicit P6-003 trust, **not** P7 peer-key enrollment or authenticated membership; do not mark P10-004 accepted until the final authorization/UX checks pass.

**Acceptance**

- Unauthorized join is never silently approved by defaults.

## P10-005 — Implement diagnostics command

- [ ] concise and detailed output.
- [ ] JSON.
- [ ] redact secrets.
- [ ] show platform/capabilities/backend.

**Acceptance**

- Diagnostics expose enough system/PAN/route/capability state to troubleshoot heterogeneous hardware.

## P10-006 — Define exit-code contract

- [x] success (0).
- [x] usage/config (2; preliminary parser).
- [x] daemon unavailable or incompatible (3).
- [ ] unsupported capability (requires complete typed error mapping).
- [ ] authorization (requires privileged CLI commands).
- [x] general operational failure (1).

**Software partial:** still needs clap parsing, daemon-error classification, and end-to-end representative exit tests before P10-006 is complete.

**Acceptance**

- Script tests verify representative exits.

---

# P11 — TUI

## P11-001 — Create Ratatui application shell

- [ ] event loop.
- [ ] daemon subscription.
- [ ] resize.
- [ ] daemon disconnect/reconnect.

**Acceptance**

- TUI starts in text-only supported Linux session and renders status.

## P11-002 — Implement overview/status screen

- [ ] node/network.
- [ ] overall health.
- [ ] device count.
- [ ] concise capability warnings.
- [ ] Internet placeholder hidden until implemented.

**Acceptance**

- Friendly default wording.

## P11-003 — Implement networks screen

- [ ] nearby/known networks.
- [ ] create/join/leave.

**Acceptance**

- Normal single-star workflow works without CLI.

## P11-004 — Implement devices screen

- [ ] direct/routed indication.
- [ ] connection state.
- [ ] details/trust actions.

**Acceptance**

- PAN role terminology is not required for normal understanding.

## P11-005 — Implement diagnostics screen

- [ ] topology.
- [ ] capabilities.
- [ ] interfaces/routes.
- [ ] errors/transitions.

**Acceptance**

- Advanced detail is available without cluttering primary UI.

## P11-006 — TUI usability review

- [ ] on-screen key hints.
- [ ] predictable back/quit.
- [ ] no mouse requirement.

**Acceptance**

- Primary actions are discoverable.

---

# P12 — Tauri desktop application

## P12-001 — Bootstrap Tauri desktop app

- [ ] Tauri 2.
- [ ] TypeScript frontend.
- [ ] React preferred unless ADR changes it.
- [ ] formatting/type-check/tests.

**Acceptance**

- App builds on initial supported Debian baseline.

## P12-002 — Implement Tauri-to-daemon bridge

- [ ] Rust backend uses `blueroute-client`.
- [ ] narrow Tauri commands.
- [ ] safe event forwarding.
- [ ] no arbitrary command execution.

**Acceptance**

- Frontend renders live daemon state without direct system D-Bus access.

## P12-003 — Design visual/component system

- [ ] typography/layout.
- [ ] status indicators.
- [ ] controls/dialogs.
- [ ] error/empty/loading states.
- [ ] accessibility basics.

**Acceptance**

- Main workflows use consistent components.

## P12-004 — Implement first-run/platform-check screen

- [ ] explain BlueRoute plainly.
- [ ] detect missing/disabled Bluetooth.
- [ ] detect unsupported required capabilities/runtime.
- [ ] offer appropriate create/join actions only when available.

**Acceptance**

- User sees actionable capability explanation, never “unsupported Chromebook” logic.

## P12-005 — Implement create-network workflow

- [ ] name/create/progress/success/error.

**Acceptance**

- Non-technical user creates working network on a capable Linux node without terminal commands.

## P12-006 — Implement nearby-network join workflow

- [ ] discovery list.
- [ ] identity presentation.
- [ ] pairing/trust prompt.
- [ ] progress/actionable errors.

**Acceptance**

- Compatible Linux node joins entirely through GUI.

## P12-007 — Implement connected-network dashboard

- [ ] friendly health.
- [ ] device list/count.
- [ ] “connected through another device” wording.
- [ ] leave.

**Acceptance**

- Dashboard tracks topology changes accurately.

## P12-008 — Implement device/details UI

- [ ] display name.
- [ ] online/offline/path.
- [ ] advanced capability/technical details.

**Acceptance**

- Friendly and diagnostic data are visually separated.

## P12-009 — Implement settings UI

- [ ] device name.
- [ ] remembered networks.
- [ ] discovery/preferences.
- [ ] advanced policy area.
- [ ] future Internet Sharing section hidden/disabled.

**Acceptance**

- Display-name change does not alter identity.

## P12-010 — Implement friendly diagnostics UI

- [ ] “Diagnose a Problem”.
- [ ] Bluetooth off/missing.
- [ ] required PAN capability unavailable.
- [ ] daemon/backend unavailable.
- [ ] permission denied.
- [ ] peer unreachable.
- [ ] address conflict.
- [ ] stale/reconnecting state.
- [ ] routed peer unavailable.
- [ ] driver/firmware quirks.

**Acceptance**

- Common UI errors point to useful guidance.

## P12-011 — Desktop accessibility pass

- [ ] keyboard navigation.
- [ ] focus states.
- [ ] semantic labels.
- [ ] contrast.
- [ ] reduced motion where relevant.

**Acceptance**

- Primary create/join/leave workflow is keyboard operable.

## P12-012 — Desktop end-to-end tests

- [ ] fake/test daemon.
- [ ] create/join.
- [ ] capability-limited states.
- [ ] reconnect/error states.

**Acceptance**

- UI regressions are testable without physical Bluetooth for every run.

---

# P13 — Reliability, reconciliation, diagnostics hardening

## P13-001 — Central desired-vs-observed reconciliation loop

- [ ] observe BlueZ.
- [ ] observe network backend.
- [ ] observe interfaces/routes.
- [ ] compare desired state.
- [ ] bounded corrective actions.

**Acceptance**

- Recovery does not depend on remembered success of prior calls.

## P13-002 — Handle BlueZ restart

- [ ] detect disappearance.
- [ ] health transition.
- [ ] re-enumerate/reconcile.

**Acceptance**

- BlueZ restart does not require BlueRoute restart.

## P13-003 — Handle NetworkManager restart

- [ ] detect disappearance.
- [ ] preserve desired state.
- [ ] reconcile after return.

**Acceptance**

- No duplicate/stale route accumulation.

## P13-004 — Handle Bluetooth adapter reset/change

- [ ] power off/on.
- [ ] adapter disappearance/reappearance.
- [ ] multiple adapters if present.
- [ ] capability refresh if active adapter changes.

**Acceptance**

- UI reports degradation/recovery correctly.

## P13-005 — Handle suspend/resume

- [ ] detect sleep/resume if needed.
- [ ] avoid retry storms.
- [ ] reconcile on resume.

**Acceptance**

- Tested suspend-capable platforms return to recoverable state.

## P13-006 — Add bounded retry/backoff

- [ ] retryable vs permanent errors.
- [ ] bounded exponential backoff.
- [ ] reset after success.

**Acceptance**

- Unreachable/unsupported peers do not busy-loop CPU/logs.

## P13-007 — Add structured journald logging

- [ ] consistent fields.
- [ ] safe IDs.
- [ ] operation correlation.
- [ ] secret redaction.

**Acceptance**

- Failed join can be diagnosed without leaking secrets.

## P13-008 — Add diagnostic snapshot model

- [ ] versions/platform.
- [ ] adapters/capabilities.
- [ ] peers/interfaces/addresses/routes/topology.
- [ ] recent errors.

**Acceptance**

- CLI/TUI/GUI consume one representation.

## P13-009 — Add support bundle export

- [ ] optional post-v1 if needed.
- [ ] redact secrets.
- [ ] user review/warning.

**Acceptance**

- Redaction tests cover all known secret fields.

---

# P14 — Security and threat model

## P14-001 — Write formal threat model

**COMPLETE — document only:** [P14-001-THREAT-MODEL.md](P14-001-THREAT-MODEL.md) inventories attacker classes, protected assets, trust boundaries, T01–T15 concrete threats, existing versus pending mitigations, residual risk, authenticated-session authorization order and negative-test/release criteria. P14-002..006 and P7 TLS remain open.

Cover:

- untrusted nearby Bluetooth devices;
- paired-but-not-member devices;
- malicious member;
- spoofed/replayed/malformed control messages;
- privilege escalation through daemon APIs;
- hostile metadata/display names;
- route injection;
- future malicious gateway.

**Acceptance**

- Threats, assumptions, mitigations, residual risk documented.

## P14-002 — Harden local D-Bus authorization

- [ ] allowed callers.
- [ ] read/write policy.
- [ ] denied-access tests.
- [ ] normal-user frontend tests.

**Acceptance**

- Arbitrary local process cannot perform unprotected privileged changes.

## P14-003 — Harden inter-node authentication

- [ ] bind session to identity.
- [ ] verify membership.
- [ ] prevent unauthenticated topology/route control.

**Software partial — provisional protocol denial only (2026-10-14):** The current control-wire decoder rejects forged route, topology, membership, and Internet-gateway command variants, including a malicious frame batched after an otherwise valid Heartbeat, without advancing session liveness. Tests pass [exact-head CI 38684997714](https://github.com/ekkus93/blueroute/actions/runs/38684997714) at `6fe80318a40b5f829b1ea0a861d09f31d9118d3e`. This is a deny-by-absence boundary until authenticated route-control semantics are designed; it does **not** verify mutual TLS, link a remote identity to the peer key, or implement authenticated routing updates. P14-003 remains open.

**Acceptance**

- Non-member cannot inject accepted routing/topology updates.

## P14-004 — Validate peer-controlled fields

- [ ] complete lengths/characters/ranges/bounds review for *all* remote metadata and route/topology fields.
- [x] prevent C0/C1 control sequences, terminal ESC and common Unicode bidirectional-override/isolate characters in shared `DisplayName` (including deserialized peer labels); preserve printable multilingual names.
- [ ] unsupported-message behavior across fully integrated authenticated daemon transport.

**Software partial:** `crates/blueroute-core/src/id.rs` input checks and negative tests passed exact-head [CI 38467939586](https://github.com/ekkus93/blueroute/actions/runs/38467939586) at `939935e`. Further reject leading/trailing raw control characters *before* trimming and Unicode line/paragraph separators, preserving printable multilingual names; committed at `acc9686c`, corrected in `43b4f697`, and qualified by [CI 38606590270](https://github.com/ekkus93/blueroute/actions/runs/38606590270) at `19472acc`. Broader peer-field fuzzing and TLS daemon integration remain open.

**Acceptance**

- Fuzz/property tests cover parser/domain conversion.

## P14-005 — Verify shell-free privileged path

- [ ] audit production code.
- [ ] isolate/remove development shell wrappers.

**Acceptance**

- Peer/user strings cannot become shell fragments.

## P14-006 — Security review persistence

- [ ] permissions/ownership.
- [ ] symlink/path handling.
- [ ] atomic writes.
- [ ] corruption recovery.

**Acceptance**

- Sensitive state is protected under documented local threat model.

---

# P15 — Packaging and installation

## P15-001 — Define Debian package contents

- [ ] daemon/CLI/TUI/desktop.
- [ ] systemd unit.
- [ ] D-Bus policy/service.
- [ ] Polkit if required.
- [ ] desktop metadata/icons.

**Acceptance**

- Manifest documented and reproducible.

## P15-002 — Build Debian package

- [ ] automated build.
- [ ] runtime dependencies.
- [ ] standard install locations.

**Acceptance**

- Package installs on clean supported Debian baseline on more than one compatible computer model when available.

## P15-003 — First-install service behavior

- [ ] enable/start according to policy.
- [ ] do not automatically create/share network.
- [ ] GUI detects daemon and capability state.

**Acceptance**

- Installation alone exposes no Bluetooth network or Internet uplink.

## P15-004 — Upgrade test

- [ ] older fixture.
- [ ] create state.
- [ ] upgrade.
- [ ] verify migration/service.

**Acceptance**

- Supported upgrade preserves identity/membership.

## P15-005 — Uninstall cleanup test

- [ ] stop service.
- [ ] remove package-created system policy.
- [ ] verify no active routes/NAT/firewall state.
- [ ] define retained/purged user data.

**Acceptance**

- Uninstall leaves no active BlueRoute networking configuration.

## P15-006 — Define future distribution packaging boundary

- [ ] identify Debian-specific packaging vs core runtime assumptions.
- [ ] ensure package scripts do not leak into core logic.
- [ ] document what another distribution would need to provide.

**Acceptance**

- Adding another distribution does not require redesigning BlueRoute domain crates.

---

# P16 — Platform and hardware acceptance

**HARDWARE DEFERRED (2026-10-13):** This phase validates product claims across compatible Linux systems. Dell Chromebook 3100 is one test family, not the support definition. Keep required physical checkboxes open until versioned evidence is collected. P16-007 simulated policy checks may be software-qualified independently but do not substitute for device evidence. Track every physical gate in `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`.

## P16-001 — Two-node clean-install acceptance

From clean supported Linux/Debian installs:

- [ ] install packages.
- [ ] create network on node A via GUI.
- [ ] discover/join on node B via GUI.
- [ ] verify TCP/UDP/IP.
- [ ] close GUIs.
- [ ] verify network persists.

**Acceptance**

- No terminal networking commands are needed in normal workflow.

## P16-002 — Multi-client single-star acceptance

- [ ] add clients up to documented capability ceiling.
- [ ] concurrent traffic.
- [ ] leave/rejoin.
- [ ] verify UI state.

**Acceptance**

- Stable at documented **test-platform-specific** count.

## P16-003 — Routed topology acceptance

- [ ] at least two PAN segments.
- [ ] cross-segment traffic.
- [ ] route diagnostics.
- [ ] hop performance.

**Acceptance**

- Multi-hop claim has physical hardware evidence.

## P16-004 — Failure/recovery acceptance

- [ ] range loss.
- [ ] hub shutdown.
- [ ] daemon restart.
- [ ] BlueZ restart.
- [ ] NetworkManager restart.
- [ ] adapter reset.
- [ ] suspend/resume where applicable.

**Acceptance**

- Expected behavior/results recorded; recoverable cases require no manual route/interface cleanup.

## P16-005 — Resource-use acceptance

- [ ] idle/active CPU.
- [ ] memory.
- [ ] scanning impact.
- [ ] sustained transfer impact.

**Acceptance**

- Results are documented per test platform and hotspots tracked.

## P16-006 — Heterogeneous-node acceptance

- [ ] Build a BlueRoute network using at least two different Linux computer/Bluetooth-controller classes.
- [ ] Verify capability exchange.
- [ ] Verify client/hub role selection respects differences.
- [ ] Verify normal traffic.

**Acceptance**

- No Dell/Chromebook-specific assumption is required for interoperability.

## P16-007 — Client-only capability acceptance

- [ ] Simulate or use a node unable/unwilling to act as NAP.
- [ ] Verify it can still join as PANU when supported.
- [ ] Verify topology never assigns prohibited role.

**Acceptance**

- Partial capability is handled as policy, not whole-device rejection when participation is possible.

---

# P17 — Documentation and v1 release readiness

## P17-001 — User guide

- [ ] install.
- [ ] create/join/leave.
- [ ] device status.
- [ ] common recovery.
- [ ] explain compatibility requirements without requiring PAN knowledge.

**Acceptance**

- Guide does not imply Chromebook-specific product scope.

## P17-002 — Administrator/CLI guide

- [ ] commands/JSON.
- [ ] logs/diagnostics.
- [ ] topology/capabilities.

**Acceptance**

- Admin can diagnose failed/unsupported link without source code.

## P17-003 — Architecture documentation pass

- [ ] update `SPEC.md` to implemented reality.
- [ ] diagrams.
- [ ] ADR index.
- [ ] daemon/API/backend boundaries.

**Acceptance**

- No known material architecture drift.

## P17-004 — Troubleshooting guide

Cover:

- Bluetooth disabled/missing;
- adapter lacks required role/capability;
- pairing failure;
- BlueZ unavailable;
- NetworkManager unavailable;
- permission denied;
- peer unreachable;
- address conflict;
- stale/reconnecting state;
- routed peer unavailable;
- driver/firmware quirks.

**Acceptance**

- Common UI errors point to useful guidance.

## P17-005 — Define v1 support matrix

- [ ] tested Linux distributions.
- [ ] kernel baselines.
- [ ] BlueZ baselines.
- [ ] network backend/version.
- [ ] tested Bluetooth controllers and representative computer models.
- [ ] observed role/peer limitations.
- [ ] known quirks.
- [ ] distinguish tested from expected-compatible configurations.

**Acceptance**

- Support is described by Linux/runtime/capability requirements, with model names used only as test evidence.

## P17-006 — v1 release gate

**Final physical gate is not waived:** Prepare software release candidates, docs, package automation and draft support matrix now, but do not mark v1 ready or advertise real multi-hop/heterogeneous support until required P16 testing is complete.

- [ ] CI green.
- [ ] physical acceptance green.
- [ ] heterogeneous-node acceptance green for broad portability claim.
- [ ] security review complete for enabled features.
- [ ] docs/support matrix current.
- [ ] package clean-install/upgrade/uninstall green.
- [ ] no Internet sharing unless P18 complete.

**Acceptance**

- Release checklist records exact commit/build and tested platform identifiers.

---

# P18 — Future Internet gateway support

**Post-core-product. Do not let this block the initial BlueRoute LAN.**

## P18-001 — Implement external connectivity detector

- [ ] distinguish link from verified Internet reachability.
- [ ] observe uplink changes.
- [ ] do not hard-code Wi-Fi as only uplink type.

**Acceptance**

- Daemon can report external connectivity without offering it to peers.

## P18-002 — Implement gateway opt-in policy

- [ ] share/do-not-share setting.
- [ ] default off.
- [ ] persist explicit preference.
- [ ] clear current state.

**Acceptance**

- Having Internet never automatically enables sharing.

## P18-003 — Extend control plane with gateway advertisement

- [ ] availability.
- [ ] willingness.
- [ ] metric/preference.
- [ ] freshness/withdrawal.

**Acceptance**

- Advertisements are authenticated member messages.

## P18-004 — Implement gateway route selection

- [ ] default-route candidates.
- [ ] path cost/preference.
- [ ] withdrawal/failure.

**Acceptance**

- Core tests cover zero/one/multiple gateways.

## P18-005 — Implement IPv4 forwarding/NAT backend

- [ ] choose NetworkManager shared mode or explicit nftables based on topology evidence.
- [ ] forwarding.
- [ ] NAT/masquerade.
- [ ] scoped firewall rules.
- [ ] cleanup.

**Acceptance**

- Direct client reaches Internet through opted-in gateway.
- Disable removes gateway-owned state.

## P18-006 — Implement DNS behavior

- [ ] DNS source/forwarding.
- [ ] gateway change.
- [ ] stale DNS cleanup.

**Acceptance**

- Client resolves names and uses gateway Internet.

## P18-007 — Prove routed-client Internet access

```text
client -> routing node -> gateway -> Internet
```

- [ ] multi-hop default route.
- [ ] TCP/UDP/DNS.
- [ ] performance measurement.

**Acceptance**

- Client not directly attached to gateway reaches Internet.

## P18-008 — Implement gateway failover

- [ ] two approved gateways.
- [ ] preference.
- [ ] failure detection.
- [ ] withdrawal/re-route.
- [ ] loop prevention.

**Acceptance**

- Physical test demonstrates bounded failover.

## P18-009 — Add desktop Internet Sharing UI

- [ ] simple opt-in.
- [ ] explain shared connection.
- [ ] show provider on clients.
- [ ] unavailable/failover states.
- [ ] low-level details only in advanced view.

**Acceptance**

- Non-technical user can enable/disable and understand state.

## P18-010 — Internet gateway security review

- [ ] firewall exposure.
- [ ] malicious clients/gateways.
- [ ] DNS trust.
- [ ] route injection.
- [ ] accidental uplink exposure.

**Acceptance**

- Threat model updated before production gateway release.

---

# P19 — Optional advanced work

## P19-001 — IPv6/ULA support

- [ ] per-network ULA prefix.
- [ ] per-segment prefixes.
- [ ] routed IPv6.
- [ ] evaluate IPv6 Internet gateway separately.

## P19-002 — Invitation code / QR enrollment

- [ ] secure invitation representation.
- [ ] desktop QR workflow where possible.
- [ ] expiry/revocation.

## P19-003 — Topology visualization

- [ ] advanced GUI graph.
- [ ] direct/routed links.
- [ ] health/path visualization.

## P19-004 — Battery-aware topology policy

- [ ] AC/battery detection where available.
- [ ] prefer powered nodes when useful.
- [ ] anti-flap behavior.

## P19-005 — Implement/evaluate non-NetworkManager Linux backend

- [ ] systemd-networkd or direct netlink option.
- [ ] implement behind P4 backend boundary.
- [ ] run common backend contract tests.

**Acceptance**

- Core/topology/front ends require no changes for alternate backend.

## P19-006 — Additional Linux distribution support

- [ ] define support criteria.
- [ ] packaging/service integration.
- [ ] run capability and network acceptance matrix.

## P19-007 — Remote management API

- [ ] only with concrete use case.
- [ ] independent authentication/security design.
- [ ] never expose local privileged D-Bus directly over network.

## P19-008 — Multiple Bluetooth adapter policy

- [ ] enumerate multiple local adapters.
- [ ] choose/prefer adapter by capability/policy.
- [ ] evaluate whether multiple radios may be used concurrently.

## P19-009 — Hardware capability database/cache

- [ ] Evaluate whether measured local capabilities should be cached.
- [ ] Never substitute model-name lookup for runtime validation where validation is possible.
- [ ] Version/invalidate cached evidence when kernel/firmware/BlueZ changes.

---

# Cross-cutting acceptance checklist

For every networking feature:

- [ ] Is the operation idempotent?
- [ ] Does failure preserve enough state for reconciliation?
- [ ] Does cleanup remove only BlueRoute-owned state?
- [ ] Is behavior testable without hardware where possible?
- [ ] Is physical evidence recorded where hardware behavior matters?
- [ ] Are hardware differences represented as capabilities rather than model-specific branches?
- [ ] Are errors typed and useful in diagnostics?
- [ ] Are secrets absent from logs?
- [ ] Do CLI/TUI/desktop consume the same daemon state?
- [ ] Does the change preserve network-backend abstraction?
- [ ] Does the change preserve future Internet-gateway separation?
- [ ] Does the default GUI avoid unnecessary networking jargon?

# Current dependency-first execution order

**Effective October 13, 2026.** Keep P0–P19 identifiers stable; completed tasks stay completed, and hardware acceptance remains open. Before each task, reconcile exact current `master`, existing PRs/branches and software evidence via Ralph Bridge. Do not create a branch/PR per task.

1. **Reconcile TODO and evidence:** P6-004 partial, P6-005 complete, PR #41 in progress, P1 and later hardware requirements deferred. Do not redo completed P4/P5/P6 hardware work.
2. **P6-006 software portion:** Review, test, fix and merge existing PR #41 if qualified. Record its physical ping/SSH/TCP/UDP/DNS/cleanup checks as deferred, not passed.
3. **P7-001 → P7-002 → P7-003 → P7-004:** Choose authenticated control transport, frame/freshness scheme, verified peer/capability handshake and session lifecycle. Unit, integration, malicious-peer and lifecycle tests are mandatory.
4. **P7-005 and early P14-001/P14-003:** Fuzz/property-test control parser; establish the inter-node threat/authorization model before accepting remote route/topology changes.
5. **P6-004:** Complete production PANU, IPv4, authenticated session, durable membership and reverse-order rollback. Mocked and simulated software acceptance now; real two-node join deferred.
6. **P6-008 → P6-009 → P6-010:** Leave, daemon restart reconciliation and link-loss recovery with deterministic fault injection. Defer physical range/adapter restart tests.
7. **P10-001/002/004/005/006, then P10-003:** Build daemon-backed CLI status, diagnostics, trust and stable exits early; finish lifecycle commands when daemon capabilities exist.
8. **P6-007:** Multi-client IP/capacity policy and concurrent simulated peers; physical controller client ceiling deferred.
9. **P8-001..P8-005, P8-008/009, software portions of P8-006/007:** Routed topologies, distribution/reconciliation and two-hop namespace/veth IP traffic; physical Bluetooth multi-hop traffic deferred.
10. **P9-001..P9-006:** Automatic topology planning and simulated failover; real radio hub-loss acceptance deferred.
11. **P11 → P12:** Ratatui TUI and Tauri 2 desktop using common daemon API; add frontend lint/type/e2e tests (also reconcile P0-003/P0-004).
12. **P13 → P14 → P15:** Reliability, security review, package build/install/upgrade/uninstall automation; capture remaining cross-machine hardware needs in backlog.
13. **P17-001..P17-005:** Documentation and provisional support matrix separating historical physical evidence, simulations and untested claims.
14. **Later, when hardware is available:** Revisit P1 characterization, remaining P6/P7/P8/P9/P13/P15 physical acceptance, and **P16-001..P16-007**. Test exact builds on representative systems; fix regressions and requalify.
15. **P17-006:** Release gate *after* the required physical and security evidence passes; no premature v1/general-availability claim.
16. **P18/P19:** Optional enhancements after v1 unless explicitly reprioritized.

**Policy:** After passing relevant software gates, mark the work **SOFTWARE COMPLETE / HARDWARE DEFERRED** when applicable and continue. Never close a task whose stated physical acceptance has not actually been demonstrated.

# Definition of first useful milestone

**Important:** A *software-ready candidate* (all implementation, mocks, network namespaces, CI green) is not the first useful physical milestone. The following still requires actual two-device Bluetooth PAN behavior and remains open until the hardware backlog is qualified.

The first useful milestone is a reliable daemon-managed two-node PAN in which:

- both nodes are compatible Linux systems;
- one can provide NAP and the other PANU according to discovered/tested capability;
- one creates a BlueRoute network;
- the other joins through the daemon API;
- IPv4 is automatic;
- ordinary TCP/UDP works;
- Wi-Fi is not carrying the acceptance traffic;
- no manual `bluetoothctl`, `nmcli`, `ip addr`, or `ip route` command is needed after installation;
- daemon restart reconciles the connection;
- CLI diagnostics explain platform capabilities and resulting state.

Dell Chromebook 3100 systems can satisfy this milestone as the first physical test pair, but the milestone must not introduce Chromebook-specific domain logic.

# Definition of v1 local-network complete

Local-network v1 is complete when:

- single-star workflow is reliable;
- routed interconnected-star networking has physical evidence;
- topology recovery is documented/tested;
- capability-aware role selection works;
- daemon, CLI, TUI, and Tauri use the same API;
- non-technical users can create/join/leave from desktop;
- Debian packaging/service behavior is validated;
- threat model and privilege boundaries are reviewed;
- support matrix describes Linux/runtime/capability requirements rather than one computer model;
- at least one heterogeneous hardware combination has been tested before broad portability claims;
- Internet sharing remains off/absent unless P18 is complete.

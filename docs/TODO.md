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
- A task is complete only when its acceptance criteria are satisfied.
- Hardware-dependent tasks must record the computer/adapter, Linux distribution, kernel, BlueZ, network-backend versions, and evidence collected.
- Do not claim physical Bluetooth behavior from CI-only tests.
- Do not generalize one adapter's limitations into a global BlueRoute limit.
- Keep architectural invariants in `docs/SPEC.md` intact unless the specification is deliberately revised first.

## Current task status

**Status date:** 2026-08-30

| Task | Status | Current state |
| --- | --- | --- |
| P0-001 | `[x]` | Workspace structure is complete and `cargo check --workspace --locked` is green on `master`. |
| P0-002 | `[-]` | Toolchain/development policy documented; clean supported Debian validation pending. |
| P0-003 | `[x]` | Rust formatting/lint policy is configured; `cargo fmt --all -- --check` and locked Clippy with `-D warnings` are green on `master`. |
| P0-004 | `[-]` | CI runs on pull requests and `master`; green master execution and formatting-failure detection are proven, while an intentional test-failure probe is still pending. |
| P0-005 | `[x]` | Development documentation complete. |
| P1-001..P1-007 | `[!]` | Blocked pending physical Linux/Bluetooth test-system evidence. |
| P1-008 | `[x]` | Capability-matrix format complete. |
| P1-009 | `[!]` | Blocked pending a materially different second hardware class. |
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
| P4-005 | `[-]` | BlueZ Network1 PANU connect/interface mapping, bounded connect timeout/cancellation, loss observation, and idempotent disconnect are implemented; working PANU data-plane hardware acceptance remains pending. |
| P5-001 | `[-]` | API version contract and compatibility rules are implemented/tested; client-side incompatibility enforcement remains pending P5-005. |
| P5-002 | `[-]` | Semantic command/response model implemented; serialization round-trip tests remain. |
| P5-003 | `[x]` | Event model and deterministic structural tests are implemented and green in CI. |
| All other tasks | `[ ]` | Not started. |

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
- Software implementation is complete; real PANU data-plane evidence is still required before this task becomes `[x]`.
- Implementation/design notes and the hardware probe are documented in `docs/P4-005-PANU.md`.

## P4-006 — Implement NAP lifecycle adapter

- [ ] register/start NAP.
- [ ] accept PAN clients.
- [ ] observe attach/detach.
- [ ] idempotent stop/cleanup.
- [ ] return capability error cleanly if local stack cannot provide NAP.

**Acceptance**

- Rust path replaces manual P1 setup for a supported NAP/PANU pair.

## P4-007 — Implement NetworkManager backend

- [ ] enumerate relevant connections/devices.
- [ ] observe changes.
- [ ] apply BlueRoute-owned addressing.
- [ ] remove only BlueRoute-owned state.

**Acceptance**

- No production operation parses `nmcli` output.

## P4-008 — Implement route adapter

- [ ] inspect routes.
- [ ] add/update/remove BlueRoute routes.
- [ ] ownership identification.
- [ ] reconcile after backend restart.

**Acceptance**

- Repeated application is idempotent and foreign routes are preserved.

## P4-009 — Implement forwarding adapter

- [ ] per-node forwarding abstraction.
- [ ] enable only when routed topology requires it.
- [ ] keep NAT/firewall separate for gateway phase.

**Acceptance**

- Routed implementation can enable forwarding without coupling to Internet NAT.

## P4-010 — Implement system capability report

- [ ] BlueZ availability/version.
- [ ] network backend/version.
- [ ] Bluetooth adapters/controllers/drivers.
- [ ] PANU/NAP capability observations where determinable.
- [ ] forwarding capability.
- [ ] practical/configured peer ceiling.
- [ ] kernel/runtime prerequisites.

**Acceptance**

- Diagnostics explain why a system is fully supported, client-only, degraded, or unsupported.

## P4-011 — Define network-backend abstraction tests

- [ ] Fake backend contract tests.
- [ ] NetworkManager implementation conformance tests.
- [ ] Ensure topology/core never imports NetworkManager-specific types.

**Acceptance**

- A future backend can be tested against the same contract.

---

# P5 — Local daemon protocol and service

## P5-001 — Define local API version

- [x] protocol version.
- [x] compatibility rules.
- [x] D-Bus service/object/interface naming.

**Acceptance**

- Client detects incompatibility before normal commands.

## P5-002 — Define command/response types

- [x] status/capabilities.
- [x] network list/create/join/leave.
- [x] node list/get/name.
- [x] discovery.
- [x] trust/forget.
- [x] diagnostics.
- [x] reserve Internet-sharing command.
- [ ] deterministic serialization/round-trip tests.

**Acceptance**

- Types live in shared protocol crate and are serialization tested.

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

- [ ] own service name.
- [ ] expose version/status/capabilities.
- [ ] emit test event.
- [ ] reject malformed requests safely.

**Acceptance**

- Test client queries daemon and receives events.

## P5-005 — Implement `blueroute-client`

- [ ] connect.
- [ ] version negotiation.
- [ ] typed requests.
- [ ] events.
- [ ] reconnect after daemon restart.

**Acceptance**

- CLI/TUI/Tauri share the crate with no duplicate system-D-Bus networking logic.

## P5-006 — Add systemd service

- [ ] unit file.
- [ ] restart policy.
- [ ] startup ordering.
- [ ] journald logging.

**Acceptance**

- Daemon starts at boot on supported Debian baseline and survives GUI logout.

## P5-007 — Define D-Bus/Polkit authorization policy

- [ ] read-only vs mutating operations.
- [ ] authorization behavior.
- [ ] no blanket front-end root access.

**Acceptance**

- Intended users can inspect status; unauthorized sensitive changes fail safely.

---

# P6 — Single-star BlueRoute LAN

This is the first complete product slice.

## P6-001 — Implement create-network operation

- [ ] create logical network ID/name.
- [ ] persist membership.
- [ ] select local NAP only if capability permits.
- [ ] establish local subnet.

**Acceptance**

- `CreateNetwork` yields stable daemon state on a NAP-capable Linux node.
- Unsupported NAP capability produces a clear error rather than a model-name special case.

## P6-002 — Implement discoverable BlueRoute network identity

- [ ] identify nearby BlueRoute-capable peers/networks.
- [ ] do not rely on display/Bluetooth name for security.
- [ ] document limitations.

**Acceptance**

- Second compatible Linux node discovers a candidate network without manual MAC entry.

## P6-003 — Implement join approval/trust workflow

- [ ] pair if needed.
- [ ] BlueRoute membership approval.
- [ ] persist accepted membership.
- [ ] reject unauthorized nodes.

**Acceptance**

- Joining requires intended trust under initial security model.

## P6-004 — Implement join-network operation

- [ ] establish PANU link.
- [ ] obtain/assign IP.
- [ ] start control session.
- [ ] mark joined only after required networking succeeds.

**Acceptance**

- Compatible node joins without manual Bluetooth/network shell commands.

## P6-005 — Implement IPv4 allocation for one star

- [ ] select private subnet policy.
- [ ] detect route conflicts.
- [ ] assign addresses.
- [ ] avoid unnecessary durable transient state.

**Acceptance**

- Repeated create/join cycles do not accumulate conflicts.

## P6-006 — Prove ordinary application traffic

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

## P7-001 — Choose control-plane transport

- [ ] evaluate transport over established IP PAN.
- [ ] port/address discovery.
- [ ] authentication binding to BlueRoute identity.
- [ ] ADR.

**Acceptance**

- Source IP is not sole identity proof.

## P7-002 — Define control protocol envelope

- [ ] version.
- [ ] sender/network identity.
- [ ] message type.
- [ ] bounded payload.
- [ ] freshness/replay approach.

**Acceptance**

- Malformed/oversized/unknown-critical data is safely rejected.

## P7-003 — Implement peer hello/capability exchange

- [ ] software/protocol version.
- [ ] stable identity.
- [ ] node capabilities.
- [ ] current segment/topology facts.

**Acceptance**

- Peers negotiate capability without knowing each other's computer model.

## P7-004 — Implement control-session lifecycle

- [ ] establish after data-plane availability.
- [ ] detect loss.
- [ ] reconnect.
- [ ] avoid duplicate logical-peer sessions.

**Acceptance**

- Repeated PAN reconnects do not leak sessions/tasks.

## P7-005 — Fuzz/property-test parser

- [ ] malformed-input tests.
- [ ] round-trip tests.
- [ ] compatibility tests.

**Acceptance**

- Tested invalid peer data cannot panic daemon.

---

# P8 — Routed interconnected-star networking

## P8-001 — Build neighbor observation model

- [ ] direct PAN links.
- [ ] candidate reachability separate from active links.
- [ ] timestamps/expiry.
- [ ] capability annotations.

**Acceptance**

- Physical/candidate graph is distinct from active routed graph.

## P8-002 — Implement distinct subnet allocation per PAN segment

- [ ] non-overlapping prefixes.
- [ ] host-route conflict detection.
- [ ] safe prefix reclamation.

**Acceptance**

- Multiple PAN stars coexist without address overlap.

## P8-003 — Implement forwarding on routing nodes

- [ ] enable IPv4 forwarding only when required.
- [ ] preserve host security policy.
- [ ] cleanup when unnecessary.

**Acceptance**

- Routing node forwards BlueRoute traffic between test PAN segments.

## P8-004 — Implement route computation v1

- [ ] compute paths.
- [ ] deterministic desired route set.
- [ ] stable-path preference.
- [ ] unreachable detection.
- [ ] respect node capability restrictions.

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

## P10-001 — Create CLI skeleton with clap

- [ ] global options.
- [ ] daemon handling.
- [ ] version.
- [ ] consistent errors/exits.

**Acceptance**

- CLI contains no direct BlueZ/NetworkManager networking code.

## P10-002 — Implement status commands

- [ ] `blueroute status`.
- [ ] `blueroute capability show`.
- [ ] `blueroute node list/show`.
- [ ] `blueroute network list`.

**Acceptance**

- Human output is useful and JSON is stable/tested.

## P10-003 — Implement network lifecycle commands

- [ ] create.
- [ ] join.
- [ ] leave.
- [ ] discover.

**Acceptance**

- Single-star network can be managed end-to-end through daemon.

## P10-004 — Implement trust commands

- [ ] pending requests where applicable.
- [ ] approve/trust.
- [ ] forget/revoke.

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

- [ ] success.
- [ ] usage/config.
- [ ] daemon unavailable.
- [ ] unsupported capability.
- [ ] authorization.
- [ ] operational failure.

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
- [ ] advanced/copy report.

**Acceptance**

- Common failures provide next actions, not raw exceptions only.

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

**Acceptance**

- Non-member cannot inject accepted routing/topology updates.

## P14-004 — Validate peer-controlled fields

- [ ] lengths/characters/ranges/bounds.
- [ ] unsupported-message behavior.

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

This phase validates product claims across compatible Linux systems. Dell Chromebook 3100 is one test family, not the support definition.

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

# Initial recommended execution order

1. `P0-*` — project baseline.
2. `P1-001` through `P1-004` — prove PAN and Linux ownership boundaries on available hardware.
3. `P2-*` and `P3-*` — hardware-independent domain/persistence model.
4. `P4-*` — production BlueZ/NetworkManager adapters and capability reporting.
5. `P5-*` — daemon/API.
6. `P6-*` — managed single-star LAN.
7. `P7-*` and `P8-*` — authenticated control and routed stars.
8. `P9-*` — automatic capability-aware topology.
9. `P10-*`, `P11-*`, `P12-*` — CLI/TUI/Tauri.
10. `P13-*` through `P17-*` — hardening, security, packaging, heterogeneous platform acceptance, v1.
11. `P18-*` — Internet gateway.
12. `P19-*` — optional extensions/backends/distributions.

# Definition of first useful milestone

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

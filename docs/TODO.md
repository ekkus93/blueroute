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
- Tasks should be completed on the smallest sensible branch/commit scope.
- A task is not complete merely because code exists; its acceptance criteria must be satisfied.
- Hardware-dependent tasks must record the hardware, Debian/kernel/BlueZ/NetworkManager versions, and the evidence collected.
- Do not claim Bluetooth hardware behavior from CI-only tests.
- Keep the architectural invariants in `docs/SPEC.md` intact unless the spec is deliberately updated first.

## Release strategy

The work is deliberately layered:

1. Prove standard Bluetooth PAN/BNEP on the reference hardware.
2. Build the Rust domain model and Linux adapters.
3. Make a daemon own the network lifecycle.
4. Deliver a reliable single-star BlueRoute LAN.
5. Add routed interconnected-star topology.
6. Add the CLI, TUI, and polished Tauri experience around the same daemon API.
7. Harden, package, and validate on Dell Chromebook 3100 hardware.
8. Add optional Internet gateway support only after the local routed network is solid.

---

# P0 — Repository and engineering baseline

## P0-001 — Define Rust workspace

- [ ] Create root Cargo workspace.
- [ ] Add initial crate/app directories:
  - `crates/blueroute-core`
  - `crates/blueroute-protocol`
  - `crates/blueroute-linux`
  - `crates/blueroute-client`
  - `apps/daemon`
  - `apps/cli`
  - `apps/tui`
  - `apps/desktop`
- [ ] Ensure workspace builds with placeholder crates.

**Acceptance**

- `cargo check --workspace` succeeds.
- No application contains duplicated domain types that should live in shared crates.

## P0-002 — Pin development toolchain policy

- [ ] Add `rust-toolchain.toml` or documented minimum stable Rust version.
- [ ] Document required Debian development packages.
- [ ] Document Node/package-manager requirements for Tauri frontend work when introduced.

**Acceptance**

- A clean Debian 13 machine can follow documented setup steps to run the Rust checks.

## P0-003 — Establish formatting and linting

- [ ] Configure rustfmt policy.
- [ ] Configure clippy policy.
- [ ] Add frontend formatting/linting policy when frontend is created.

**Acceptance**

- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets` passes under the agreed warning policy.

## P0-004 — Establish CI baseline

- [ ] Add GitHub Actions workflow for Rust formatting, clippy, tests, and build.
- [ ] Add frontend checks when Tauri code exists.
- [ ] Cache dependencies without hiding lockfile problems.

**Acceptance**

- CI runs on pull requests and `master`.
- A deliberately broken format/test change is caught by CI.

## P0-005 — Add contribution/development documentation

- [ ] Add developer quick-start instructions.
- [ ] Explain which tests require physical Bluetooth hardware.
- [ ] Document project task-ID convention.

**Acceptance**

- A contributor can distinguish deterministic CI tests from hardware acceptance tests.

---

# P1 — Reference hardware characterization

Do this early. The architecture must be informed by what the Dell Chromebook 3100 Bluetooth hardware actually supports.

## P1-001 — Inventory reference Chromebook hardware

- [ ] Record exact Dell Chromebook 3100 model variants available.
- [ ] Record Bluetooth controller/chipset identifiers.
- [ ] Record kernel driver/module.
- [ ] Record Debian, kernel, BlueZ, and NetworkManager versions.
- [ ] Capture relevant `bluetoothctl show`, `btmgmt info`, `lsusb`/`lspci`, and system information.

**Acceptance**

- A versioned hardware report exists under `docs/hardware/`.

## P1-002 — Verify basic Bluetooth stability on Debian 13

- [ ] Confirm adapter powers on/off reliably.
- [ ] Confirm discovery works.
- [ ] Pair two Chromebook 3100 units.
- [ ] Disconnect/reconnect repeatedly.
- [ ] Test after reboot.

**Acceptance**

- Reproducible pairing and reconnection procedure is documented.
- Any driver/firmware quirks are recorded.

## P1-003 — Prove manual PANU/NAP connectivity

- [ ] Create one NAP and one PANU using existing Linux tooling/APIs.
- [ ] Confirm BNEP interface creation.
- [ ] Assign working IPv4 configuration.
- [ ] Ping both directions.
- [ ] Run TCP traffic (SSH, iperf3, or equivalent).
- [ ] Run UDP traffic.

**Acceptance**

- Two Chromebook 3100 units exchange ordinary TCP/IP over Bluetooth PAN with Wi-Fi disabled.
- Exact commands/system state are documented.

## P1-004 — Determine NetworkManager vs direct BlueZ responsibility

- [ ] Prototype PAN setup through NetworkManager.
- [ ] Prototype/inspect direct BlueZ network APIs where relevant.
- [ ] Record which component should own PAN profile creation and which should own IP configuration.
- [ ] Create an ADR for the selected production boundary.

**Acceptance**

- Production path is chosen for NAP registration, PANU connection, address configuration, and teardown.
- Choice is justified by reproducible testing on Debian 13.

## P1-005 — Measure single-link baseline

- [ ] Measure TCP throughput.
- [ ] Measure UDP behavior.
- [ ] Measure latency and packet loss.
- [ ] Measure CPU and memory impact.
- [ ] Test sustained transfer.
- [ ] Repeat at several practical distances.

**Acceptance**

- Baseline results are stored in `docs/hardware/` with test method and versions.

## P1-006 — Measure simultaneous PAN connection limit

- [ ] Add PANU clients one at a time to a NAP.
- [ ] Record successful connection count.
- [ ] Run simultaneous traffic rather than testing idle links only.
- [ ] Record instability/failure modes at and beyond practical limits.

**Acceptance**

- BlueRoute has an evidence-based initial maximum/recommended peer count for the reference hardware.
- No hard-coded limit is introduced without a configuration/capability abstraction.

## P1-007 — Test suspend/resume and range loss

- [ ] Suspend NAP and PANU separately.
- [ ] Resume and observe BlueZ/NetworkManager state.
- [ ] Move a node out of range and back.
- [ ] Record stale interfaces/profiles/routes if any.

**Acceptance**

- Recovery requirements are documented for the daemon reconciliation design.

---

# P2 — Core domain model

## P2-001 — Define stable identifiers

- [ ] Define `NodeId`.
- [ ] Define `NetworkId`.
- [ ] Define `LinkId`/segment identity if required.
- [ ] Keep human-readable names separate from authorization identity.

**Acceptance**

- IDs serialize deterministically.
- Display names can change without changing identity.
- Unit tests cover parsing/serialization/invalid values.

## P2-002 — Define node capability model

- [ ] Model Bluetooth/PAN capability.
- [ ] Model routing capability.
- [ ] Model controller connection limits.
- [ ] Reserve external-connectivity and gateway-sharing capabilities.
- [ ] Keep “has Internet” distinct from “willing to share Internet.”

**Acceptance**

- Capabilities can represent the known Chromebook constraints without UI-specific types.

## P2-003 — Define network membership model

- [ ] Model local membership state.
- [ ] Model trusted/known peers.
- [ ] Model network name vs network identity.
- [ ] Model joining/leaving transitions.

**Acceptance**

- Unit tests cover legal and illegal transitions.

## P2-004 — Define link and PAN-segment model

- [ ] Represent PANU/NAP relationship without exposing it as the default UI vocabulary.
- [ ] Represent link state, health, and observed properties.
- [ ] Represent a PAN segment separately from the overall BlueRoute network.

**Acceptance**

- One-star and multi-star graphs can both be represented.

## P2-005 — Define topology graph

- [ ] Add graph operations for nodes and links.
- [ ] Track direct vs routed reachability.
- [ ] Represent unavailable/failed links.
- [ ] Add deterministic graph snapshots for diagnostics/tests.

**Acceptance**

- Unit tests cover connect/disconnect, partitions, and multiple paths.

## P2-006 — Define route model

- [ ] Represent prefix/segment destinations.
- [ ] Represent next hop and metric/cost.
- [ ] Reserve default/Internet destination semantics.
- [ ] Track ownership so BlueRoute can remove only its own routes.

**Acceptance**

- Route model supports local-only release and future gateway release without breaking changes.

## P2-007 — Define health model

- [ ] Define healthy/degraded/reconnecting/error concepts.
- [ ] Separate adapter, membership, link, topology, and gateway health.
- [ ] Add aggregation rules for user-facing status.

**Acceptance**

- A disconnected optional peer does not necessarily mark the entire network fatal.
- Tests cover representative combinations.

## P2-008 — Define typed error taxonomy

- [ ] Add domain errors listed in `SPEC.md`.
- [ ] Preserve low-level diagnostic context without forcing UI to show raw D-Bus errors.

**Acceptance**

- Front ends can map errors to friendly messages while diagnostics retain actionable detail.

---

# P3 — Configuration and persistence

## P3-001 — Define configuration schema

- [ ] Define daemon settings.
- [ ] Define node display name.
- [ ] Define address pool policy.
- [ ] Define topology policy placeholders.
- [ ] Reserve gateway settings disabled by default.

**Acceptance**

- Configuration is versioned and validated.
- Unknown/invalid fields have defined behavior.

## P3-002 — Implement stable node identity persistence

- [ ] Generate identity at first run.
- [ ] Persist securely.
- [ ] Recover after daemon restart.

**Acceptance**

- Rebooting does not create a new logical node.

## P3-003 — Implement network membership persistence

- [ ] Persist known networks.
- [ ] Persist trusted peer membership data.
- [ ] Define cleanup/forget semantics.

**Acceptance**

- Joining then rebooting preserves the intended remembered-network state.

## P3-004 — Add schema migration framework

- [ ] Version persistent format.
- [ ] Add migration entry points.
- [ ] Test at least one synthetic old-to-new migration.

**Acceptance**

- Future schema evolution does not require deleting all user state.

## P3-005 — Secure persistent secrets

- [ ] Identify which fields are secret.
- [ ] Enforce restrictive permissions.
- [ ] Ensure logs/debug serialization redact them.

**Acceptance**

- Automated tests verify redaction and file-permission expectations where feasible.

---

# P4 — Linux system adapters

## P4-001 — Create adapter trait boundaries

- [ ] Define Bluetooth backend trait(s).
- [ ] Define network/IP backend trait(s).
- [ ] Define clock/event abstractions needed for deterministic tests.
- [ ] Keep core free of direct D-Bus types.

**Acceptance**

- Core topology/state tests run entirely with fake adapters.

## P4-002 — Implement BlueZ service/adapter discovery

- [ ] Connect via Rust D-Bus library (expected: zbus or equivalent).
- [ ] Enumerate adapters.
- [ ] Observe power state.
- [ ] Subscribe to adapter changes.

**Acceptance**

- Daemon-side probe reports adapter state without shelling out to `bluetoothctl`.

## P4-003 — Implement Bluetooth device discovery adapter

- [ ] Start/stop discovery.
- [ ] Observe device add/change/remove events.
- [ ] Map BlueZ device properties into domain types.
- [ ] Bound/clean discovery state.

**Acceptance**

- Nearby test Chromebook appears through the Rust adapter.

## P4-004 — Implement pairing/trust adapter

- [ ] Initiate pairing.
- [ ] Handle pairing callbacks/agent requirements.
- [ ] Mark/unmark BlueZ trust as policy requires.
- [ ] Map rejection/timeouts into typed errors.

**Acceptance**

- Two test Chromebooks can complete pairing through Rust-controlled flow.

## P4-005 — Implement PANU connection adapter

- [ ] Establish PANU connection through selected production API.
- [ ] Identify resulting network interface.
- [ ] Observe connection loss.
- [ ] Disconnect idempotently.

**Acceptance**

- Rust integration test/manual harness creates working PANU data plane on hardware.

## P4-006 — Implement NAP lifecycle adapter

- [ ] Register/start NAP.
- [ ] Accept PAN clients.
- [ ] Observe client attach/detach.
- [ ] Stop/cleanup idempotently.

**Acceptance**

- Rust integration path replaces the manual P1 setup for one NAP and one PANU.

## P4-007 — Implement NetworkManager state adapter

- [ ] Enumerate relevant connections/devices.
- [ ] Observe state changes.
- [ ] Apply BlueRoute-owned address configuration.
- [ ] Remove only BlueRoute-owned configuration.

**Acceptance**

- No production operation depends on parsing `nmcli` output.

## P4-008 — Implement route adapter

- [ ] Inspect effective routes.
- [ ] Add/update/remove BlueRoute routes.
- [ ] Tag/identify ownership where supported.
- [ ] Reconcile after NetworkManager restart.

**Acceptance**

- Repeated application is idempotent.
- Non-BlueRoute routes are preserved.

## P4-009 — Implement forwarding adapter

- [ ] Add abstraction for per-node IP forwarding.
- [ ] Keep it disabled unless routed topology requires it.
- [ ] Reserve NAT/firewall methods separately for gateway phase.

**Acceptance**

- Multi-hop implementation can enable forwarding without baking NAT into the same operation.

## P4-010 — Implement system capability report

- [ ] Detect BlueZ availability/version.
- [ ] Detect NetworkManager availability/version.
- [ ] Detect Bluetooth adapter availability.
- [ ] Detect required kernel/network capabilities where practical.

**Acceptance**

- Diagnostics can explain unsupported/missing system prerequisites.

---

# P5 — Local daemon protocol and service

## P5-001 — Define local API version

- [ ] Define protocol version type.
- [ ] Define compatibility rules.
- [ ] Define service/object/interface naming.

**Acceptance**

- A client can detect incompatible daemon API before issuing normal commands.

## P5-002 — Define command/response types

- [ ] `GetStatus`
- [ ] `ListNetworks`
- [ ] `CreateNetwork`
- [ ] `JoinNetwork`
- [ ] `LeaveNetwork`
- [ ] `ListNodes`
- [ ] `GetNode`
- [ ] `SetDeviceName`
- [ ] discovery controls
- [ ] trust/forget operations
- [ ] diagnostics
- [ ] reserve future Internet-sharing operation

**Acceptance**

- Types live in shared protocol crate and are serialization tested.

## P5-003 — Define event types

- [ ] Network discovery events.
- [ ] Peer/node changes.
- [ ] Membership changes.
- [ ] Link/topology changes.
- [ ] Health changes.
- [ ] Authorization errors.
- [ ] Reserve gateway/Internet events.

**Acceptance**

- Event snapshots are deterministic enough for UI tests.

## P5-004 — Implement daemon D-Bus service skeleton

- [ ] Own service name.
- [ ] Expose version/status method.
- [ ] Emit test signal/event.
- [ ] Gracefully reject malformed requests.

**Acceptance**

- A standalone test client can query daemon version and receive an event.

## P5-005 — Implement `blueroute-client`

- [ ] Connect to local daemon.
- [ ] Negotiate/check API version.
- [ ] Provide typed request helpers.
- [ ] Subscribe to events.
- [ ] Reconnect after daemon restart where sensible.

**Acceptance**

- CLI/TUI/Tauri can share this crate with no direct D-Bus duplication.

## P5-006 — Add systemd service

- [ ] Create unit file.
- [ ] Define restart policy.
- [ ] Define startup ordering around BlueZ/NetworkManager.
- [ ] Log to journald.

**Acceptance**

- Daemon starts at boot on Debian test system and survives GUI logout.

## P5-007 — Define D-Bus/Polkit authorization policy

- [ ] Identify read-only operations.
- [ ] Identify mutating operations.
- [ ] Define authorization behavior.
- [ ] Avoid granting front ends blanket root access.

**Acceptance**

- Read status works for intended local users.
- Sensitive changes fail safely without authorization.

---

# P6 — Single-star BlueRoute LAN

This is the first complete product slice.

## P6-001 — Implement create-network operation

- [ ] Create logical `NetworkId` and name.
- [ ] Persist membership.
- [ ] Start local NAP role as policy selects.
- [ ] Establish local subnet configuration.

**Acceptance**

- `CreateNetwork` results in a stable, inspectable daemon state on one Chromebook.

## P6-002 — Implement discoverable BlueRoute network identity

- [ ] Select initial mechanism for identifying nearby BlueRoute-capable peers/networks.
- [ ] Do not rely on user-visible Bluetooth name alone for security.
- [ ] Document limitations of initial discovery method.

**Acceptance**

- A second Chromebook can identify a nearby candidate BlueRoute network without manual MAC-address entry.

## P6-003 — Implement join approval/trust workflow

- [ ] Pair if required.
- [ ] Perform BlueRoute-level membership approval.
- [ ] Persist accepted membership.
- [ ] Reject unauthorized nodes cleanly.

**Acceptance**

- Joining requires explicit intended trust in the initial security model.

## P6-004 — Implement join-network operation

- [ ] Establish PANU link.
- [ ] Obtain/assign IP configuration.
- [ ] Start control-plane session.
- [ ] Mark joined only after required networking succeeds.

**Acceptance**

- A second Chromebook joins without manual Bluetooth/network shell commands.

## P6-005 — Implement IPv4 address allocation for one star

- [ ] Select initial private subnet policy.
- [ ] Detect collision with active local routes.
- [ ] Assign NAP/PANU addresses.
- [ ] Persist only what needs to be durable.

**Acceptance**

- Repeated create/join cycles do not accumulate conflicting addresses.

## P6-006 — Prove ordinary application traffic

- [ ] Ping.
- [ ] SSH.
- [ ] TCP bulk transfer.
- [ ] UDP test.
- [ ] Verify DNS/name behavior separately from raw IP connectivity.

**Acceptance**

- Applications require no BlueRoute-specific networking library.

## P6-007 — Add third and subsequent clients

- [ ] Connect multiple PANU nodes up to conservative tested limit.
- [ ] Exercise concurrent traffic.
- [ ] Surface NAP capacity in diagnostics.

**Acceptance**

- Stable multi-client star works on reference hardware at the documented supported count.

## P6-008 — Implement leave-network operation

- [ ] Tear down membership runtime state.
- [ ] Remove BlueRoute-owned addresses/routes/profiles as policy dictates.
- [ ] Preserve remembered trust only when intended.

**Acceptance**

- Leave is idempotent and leaves no stale BlueRoute data-plane state.

## P6-009 — Implement daemon restart reconciliation

- [ ] Restart daemon while PAN links exist.
- [ ] Re-observe system state.
- [ ] Reconcile instead of blindly duplicating connections/routes.

**Acceptance**

- Daemon restart does not require reboot or manual network cleanup.

## P6-010 — Implement link-loss reconnect

- [ ] Detect lost PAN peer.
- [ ] Transition state to degraded/reconnecting.
- [ ] Retry with bounded backoff.
- [ ] Restore healthy state when peer returns.

**Acceptance**

- Range-loss/recovery test succeeds without manual commands.

---

# P7 — Inter-node control plane

## P7-001 — Choose control-plane transport

- [ ] Evaluate transport over established IP PAN.
- [ ] Define port/address discovery policy.
- [ ] Define authentication binding to BlueRoute identity.
- [ ] Write ADR.

**Acceptance**

- Choice does not treat source IP as sole identity proof.

## P7-002 — Define control protocol envelope

- [ ] Version.
- [ ] Sender identity.
- [ ] Network identity.
- [ ] Message type.
- [ ] Bounded payload.
- [ ] Replay/freshness strategy as required.

**Acceptance**

- Parser rejects oversized, malformed, and unknown-critical data safely.

## P7-003 — Implement peer hello/capability exchange

- [ ] Exchange software/protocol version.
- [ ] Exchange stable node identity.
- [ ] Exchange capabilities.
- [ ] Exchange current segment/topology facts needed by next phase.

**Acceptance**

- Daemon status shows authenticated peer identity independently of IP/MAC display name.

## P7-004 — Implement control-session lifecycle

- [ ] Establish session after data-plane availability.
- [ ] Detect session loss.
- [ ] Reconnect.
- [ ] Avoid duplicate active sessions for the same logical peer.

**Acceptance**

- Repeated PAN reconnects do not leak sessions/tasks.

## P7-005 — Fuzz/property-test protocol parser

- [ ] Add bounded malformed-input tests.
- [ ] Add serialization round-trip tests.
- [ ] Add version-compatibility tests.

**Acceptance**

- Invalid peer data cannot panic the daemon in tested cases.

---

# P8 — Routed interconnected-star networking

## P8-001 — Build neighbor observation model

- [ ] Record which peers have direct PAN links.
- [ ] Record candidate reachability observations separately from active links.
- [ ] Timestamp/expire stale observations.

**Acceptance**

- Core can represent physical/candidate graph separately from active routed graph.

## P8-002 — Implement distinct subnet allocation per PAN segment

- [ ] Allocate non-overlapping segment prefixes.
- [ ] Detect host route conflicts.
- [ ] Reclaim retired segment prefixes safely.

**Acceptance**

- Two simultaneous PAN stars can exist without overlapping local addresses.

## P8-003 — Implement forwarding on routing nodes

- [ ] Enable IPv4 forwarding only when required.
- [ ] Preserve host security policy.
- [ ] Disable/cleanup when no longer needed.

**Acceptance**

- Router node forwards BlueRoute traffic between two test PAN segments.

## P8-004 — Implement route computation v1

- [ ] Compute paths across active topology.
- [ ] Produce deterministic desired route set per node.
- [ ] Prefer stable paths over unnecessary churn.
- [ ] Detect unreachable destinations.

**Acceptance**

- Unit tests cover line, tree, redundant, and partitioned graphs.

## P8-005 — Implement route distribution/application

- [ ] Deliver desired routing information through control plane.
- [ ] Apply routes through Linux adapter.
- [ ] Remove stale routes.
- [ ] Reject routes for foreign/non-member networks.

**Acceptance**

- Route table matches expected topology after connect and disconnect events.

## P8-006 — Prove two-hop TCP/IP

Test topology:

```text
client A -> hub/router B -> client C
```

- [ ] Ping end to end.
- [ ] TCP transfer end to end.
- [ ] UDP traffic end to end.
- [ ] Record throughput/latency versus direct link.

**Acceptance**

- A and C exchange ordinary IP traffic despite lacking a direct PAN link.

## P8-007 — Prove interconnected stars

Test topology:

```text
A/B clients -> hub 1 -> hub 2 -> C/D clients
```

- [ ] Establish two PAN segments.
- [ ] Route across hubs.
- [ ] Exercise traffic in both directions.

**Acceptance**

- Cross-star traffic works without giant layer-2 bridging.

## P8-008 — Implement route recovery after node loss

- [ ] Remove failed next-hop routes.
- [ ] Select alternate path when available.
- [ ] Mark unreachable when no path exists.
- [ ] Restore path when peer returns.

**Acceptance**

- Redundant hardware topology survives removal of one routing node when an alternate physical path exists.

## P8-009 — Evaluate dynamic routing protocol option

- [ ] Compare current BlueRoute route orchestration with Babel or another mature protocol.
- [ ] Evaluate complexity, convergence, observability, security, and hardware fit.
- [ ] Record ADR to adopt or reject for v1.

**Acceptance**

- Decision is evidence-based; no routing daemon is added merely because mesh terminology suggests one.

---

# P9 — Automatic topology management

## P9-001 — Define topology policy inputs

- [ ] Connection capacity.
- [ ] Direct-neighbor availability.
- [ ] Link quality/stability when measurable.
- [ ] Hop count/path cost.
- [ ] Current topology stability.
- [ ] Battery/power policy placeholder.
- [ ] Future gateway preference placeholder.

**Acceptance**

- Inputs are explicit and testable rather than hidden in ad hoc heuristics.

## P9-002 — Implement NAP role selection v1

- [ ] Choose hub nodes automatically in a small topology.
- [ ] Respect hardware connection capacity.
- [ ] Avoid assigning every node as a hub unnecessarily.

**Acceptance**

- Deterministic tests produce expected role assignments for representative graphs.

## P9-003 — Implement topology plan diff

- [ ] Compare desired vs active topology.
- [ ] Generate minimal ordered changes.
- [ ] Avoid tearing down a working path before replacement exists where possible.

**Acceptance**

- Small metric changes do not cause full-network churn.

## P9-004 — Implement topology executor

- [ ] Create required links.
- [ ] Remove obsolete links.
- [ ] Coordinate routing changes.
- [ ] Roll back or reconcile after partial failure.

**Acceptance**

- Injected operation failure leaves daemon able to reconcile to a valid state.

## P9-005 — Implement hub-loss reformation

- [ ] Detect hub loss.
- [ ] Select replacement hub/path.
- [ ] Reconnect members.
- [ ] Recompute routes.

**Acceptance**

- Hardware test demonstrates automatic recovery where radio reachability permits.

## P9-006 — Add topology anti-flap policy

- [ ] Hysteresis/minimum stability thresholds.
- [ ] Backoff after repeated failures.
- [ ] Prefer known-good existing links.

**Acceptance**

- Synthetic noisy metrics do not trigger continuous topology rebuild.

---

# P10 — CLI

## P10-001 — Create CLI skeleton with clap

- [ ] Global options.
- [ ] Daemon connection handling.
- [ ] Version output.
- [ ] Consistent error/exit handling.

**Acceptance**

- CLI contains no direct BlueZ/NetworkManager networking code.

## P10-002 — Implement status commands

- [ ] `blueroute status`
- [ ] `blueroute node list`
- [ ] `blueroute node show`
- [ ] `blueroute network list`

**Acceptance**

- Human-readable output is useful.
- JSON output is stable and tested for automation.

## P10-003 — Implement network lifecycle commands

- [ ] create
- [ ] join
- [ ] leave
- [ ] discover

**Acceptance**

- A single-star network can be operated end-to-end from CLI via daemon.

## P10-004 — Implement trust commands

- [ ] list pending trust requests where applicable.
- [ ] approve/trust.
- [ ] forget/revoke.

**Acceptance**

- Unauthorized joining cannot be silently approved by CLI defaults.

## P10-005 — Implement diagnostics command

- [ ] concise summary.
- [ ] detailed view.
- [ ] JSON form.
- [ ] redact secrets.

**Acceptance**

- Diagnostics expose adapters, PAN interfaces, addresses, peers, topology, and BlueRoute routes.

## P10-006 — Define exit-code contract

- [ ] success.
- [ ] usage/config error.
- [ ] daemon unavailable.
- [ ] authorization failure.
- [ ] operational failure.

**Acceptance**

- Script tests verify representative exit codes.

---

# P11 — TUI

## P11-001 — Create Ratatui application shell

- [ ] Event loop.
- [ ] daemon event subscription.
- [ ] resize handling.
- [ ] graceful daemon disconnect/reconnect.

**Acceptance**

- TUI starts on a text-only Debian session and renders daemon status.

## P11-002 — Implement overview/status screen

- [ ] local node name.
- [ ] joined network.
- [ ] overall health.
- [ ] connected device count.
- [ ] Internet state placeholder hidden until implemented.

**Acceptance**

- Screen uses friendly default wording.

## P11-003 — Implement networks screen

- [ ] nearby/known networks.
- [ ] create.
- [ ] join.
- [ ] leave.

**Acceptance**

- User can complete normal single-star workflow without CLI.

## P11-004 — Implement devices screen

- [ ] direct/routed indication.
- [ ] connection state.
- [ ] details.
- [ ] trust actions where appropriate.

**Acceptance**

- Low-level PAN role is not required to understand normal status.

## P11-005 — Implement diagnostics screen

- [ ] topology.
- [ ] interfaces/routes.
- [ ] recent errors/state transitions.

**Acceptance**

- Advanced details are available without cluttering primary screens.

## P11-006 — TUI usability/keyboard review

- [ ] On-screen key hints.
- [ ] predictable back/quit behavior.
- [ ] no mouse requirement.

**Acceptance**

- A user unfamiliar with Ratatui can discover primary actions from the screen itself.

---

# P12 — Tauri desktop application

## P12-001 — Bootstrap Tauri desktop app

- [ ] Tauri 2 project.
- [ ] TypeScript frontend.
- [ ] Choose/document frontend framework (React is preferred unless changed by ADR).
- [ ] Establish formatting/type-check/test tooling.

**Acceptance**

- Desktop app builds on Debian 13.

## P12-002 — Implement Tauri-to-daemon bridge

- [ ] Rust backend uses `blueroute-client`.
- [ ] Define narrow Tauri commands.
- [ ] Forward daemon events to frontend safely.
- [ ] Do not expose arbitrary system command execution.

**Acceptance**

- Frontend can render live daemon status without direct system D-Bus access.

## P12-003 — Design visual language and component system

- [ ] Typography.
- [ ] spacing/layout.
- [ ] status indicators.
- [ ] buttons/forms/dialogs.
- [ ] error/empty/loading states.
- [ ] accessibility basics.

**Acceptance**

- Main workflows use consistent components rather than one-off styling.

## P12-004 — Implement first-run screen

- [ ] Explain BlueRoute in plain language.
- [ ] Detect missing/disabled Bluetooth.
- [ ] Offer create/join actions.

**Acceptance**

- User is not shown PANU/NAP/BNEP terminology.

## P12-005 — Implement create-network workflow

- [ ] Name network.
- [ ] Create.
- [ ] Show progress.
- [ ] Show success/error recovery.

**Acceptance**

- Non-technical user can create a working network without terminal commands.

## P12-006 — Implement nearby-network join workflow

- [ ] Discovery list.
- [ ] Network/device identity presentation.
- [ ] pairing/trust prompt.
- [ ] join progress.
- [ ] actionable errors.

**Acceptance**

- Another Chromebook can join entirely through GUI.

## P12-007 — Implement connected-network dashboard

- [ ] friendly health summary.
- [ ] device count/list.
- [ ] direct vs “connected through another device” wording.
- [ ] leave action.

**Acceptance**

- Dashboard accurately reflects daemon state after topology changes.

## P12-008 — Implement devices/details UI

- [ ] node display name.
- [ ] online/offline.
- [ ] path/reachability.
- [ ] advanced technical detail expansion.

**Acceptance**

- Friendly and diagnostic information are visually separated.

## P12-009 — Implement settings UI

- [ ] local device name.
- [ ] remembered networks.
- [ ] discovery preferences where applicable.
- [ ] advanced settings area.
- [ ] reserve Internet Sharing section hidden/disabled until implemented.

**Acceptance**

- Changing display name does not change node identity.

## P12-010 — Implement friendly diagnostics UI

- [ ] “Diagnose a Problem” entry point.
- [ ] common problems: Bluetooth off, daemon unavailable, permission denied, peer unreachable.
- [ ] optional advanced details/copy report.

**Acceptance**

- Common failure states provide a next action rather than raw exception text only.

## P12-011 — Desktop accessibility pass

- [ ] keyboard navigation.
- [ ] focus states.
- [ ] semantic labels.
- [ ] contrast review.
- [ ] reduced-motion consideration where animations are used.

**Acceptance**

- Primary create/join/leave workflow is keyboard operable.

## P12-012 — Desktop end-to-end tests

- [ ] daemon fake/test fixture.
- [ ] create flow.
- [ ] join flow.
- [ ] reconnect state.
- [ ] error state.

**Acceptance**

- UI regressions can be caught without physical Bluetooth hardware for every frontend test.

---

# P13 — Reliability, reconciliation, and diagnostics hardening

## P13-001 — Central desired-vs-observed reconciliation loop

- [ ] Observe BlueZ state.
- [ ] Observe NetworkManager state.
- [ ] Observe interfaces/routes.
- [ ] Compare against desired BlueRoute state.
- [ ] Apply bounded corrective actions.

**Acceptance**

- Recovery does not depend solely on remembering that a prior operation succeeded.

## P13-002 — Handle BlueZ restart

- [ ] Detect service disappearance.
- [ ] Mark health appropriately.
- [ ] Re-enumerate/reconcile after return.

**Acceptance**

- BlueZ restart does not require BlueRoute daemon restart.

## P13-003 — Handle NetworkManager restart

- [ ] Detect service disappearance.
- [ ] Preserve desired state.
- [ ] Reconcile addresses/routes after return.

**Acceptance**

- No duplicate/stale BlueRoute route accumulation after test restart.

## P13-004 — Handle Bluetooth adapter reset

- [ ] adapter power off/on.
- [ ] USB/device disappearance if relevant.
- [ ] re-establish discovery and links.

**Acceptance**

- UI reports degradation and recovery correctly.

## P13-005 — Handle system suspend/resume

- [ ] detect sleep/resume events if necessary.
- [ ] stop unsafe retry storms during sleep.
- [ ] reconcile after resume.

**Acceptance**

- Reference Chromebook resumes into a recoverable BlueRoute state.

## P13-006 — Add bounded retry/backoff framework

- [ ] classify retryable vs permanent errors.
- [ ] exponential/bounded backoff.
- [ ] reset backoff after success.

**Acceptance**

- Unreachable peer does not cause busy-loop CPU/log spam.

## P13-007 — Add structured journald logging

- [ ] consistent event fields.
- [ ] node/network IDs in safe form.
- [ ] operation correlation where useful.
- [ ] secret redaction.

**Acceptance**

- A failed join can be reconstructed from logs without exposing membership secrets.

## P13-008 — Add diagnostic snapshot model

- [ ] versions.
- [ ] adapters.
- [ ] peers.
- [ ] interfaces.
- [ ] addresses.
- [ ] routes.
- [ ] topology.
- [ ] recent errors.

**Acceptance**

- CLI, TUI, and GUI consume one shared diagnostic representation.

## P13-009 — Add support bundle export

- [ ] optional post-v1 if scope requires.
- [ ] redact secrets.
- [ ] include explicit user review/warning.

**Acceptance**

- Automated redaction tests cover all known secret fields.

---

# P14 — Security and threat model

## P14-001 — Write formal threat model

Cover at least:

- untrusted nearby Bluetooth devices;
- paired-but-not-member devices;
- malicious BlueRoute member;
- spoofed control messages;
- replay;
- malformed protocol data;
- privilege escalation through daemon APIs;
- hostile display names/metadata;
- route injection;
- future malicious gateway.

**Acceptance**

- Threats, assumptions, mitigations, and residual risks are documented.

## P14-002 — Harden local D-Bus API authorization

- [ ] Define allowed callers.
- [ ] Define read/write authorization.
- [ ] Test denied access.
- [ ] Test frontend operation under normal user account.

**Acceptance**

- A random local process cannot perform privileged BlueRoute changes merely by calling an unprotected method.

## P14-003 — Harden inter-node control authentication

- [ ] Bind peer session to BlueRoute identity.
- [ ] Verify network membership.
- [ ] Prevent unauthenticated route/topology control.

**Acceptance**

- Non-member peer cannot inject accepted route/topology updates.

## P14-004 — Validate all peer-controlled fields

- [ ] lengths.
- [ ] character/display handling.
- [ ] numeric ranges.
- [ ] collection bounds.
- [ ] unsupported message behavior.

**Acceptance**

- Fuzz/property tests cover parser and representative domain conversion.

## P14-005 — Verify shell-free privileged path

- [ ] Audit production code for shelling out.
- [ ] Remove or strictly isolate development-only command wrappers.

**Acceptance**

- Peer/user strings cannot become shell fragments in privileged production paths.

## P14-006 — Security review of persistence

- [ ] permissions.
- [ ] ownership.
- [ ] symlink/path handling.
- [ ] atomic writes.
- [ ] corruption recovery.

**Acceptance**

- Security-sensitive state cannot be trivially read or replaced by unintended local users under the documented threat model.

---

# P15 — Packaging and installation

## P15-001 — Define Debian package contents

- [ ] daemon.
- [ ] CLI.
- [ ] TUI.
- [ ] desktop app.
- [ ] systemd unit.
- [ ] D-Bus policy/service files.
- [ ] Polkit files if required.
- [ ] desktop metadata/icons.

**Acceptance**

- Package manifest is documented and reproducible.

## P15-002 — Build Debian package

- [ ] Automate build.
- [ ] Declare runtime dependencies.
- [ ] Install files to standard locations.

**Acceptance**

- Package installs cleanly on a fresh Debian 13 reference system.

## P15-003 — First-install service behavior

- [ ] Enable/start daemon according to documented policy.
- [ ] Do not unexpectedly create/share a network.
- [ ] GUI detects daemon availability.

**Acceptance**

- Installation alone does not expose a Bluetooth network or Internet uplink.

## P15-004 — Upgrade test

- [ ] Install older package fixture.
- [ ] Create configuration/state.
- [ ] Upgrade.
- [ ] Verify migration and service health.

**Acceptance**

- Supported upgrade path preserves identity/membership state.

## P15-005 — Uninstall cleanup test

- [ ] Stop service.
- [ ] Remove package-created system policy files.
- [ ] Verify no active BlueRoute routes/NAT/firewall state remains.
- [ ] Define whether user data is retained or purged.

**Acceptance**

- Uninstall does not leave an active networking configuration behind.

---

# P16 — Reference hardware acceptance

## P16-001 — Two-node clean-install acceptance

Starting from clean supported Debian installs:

- [ ] Install BlueRoute packages.
- [ ] Create network on node A via GUI.
- [ ] Discover/join on node B via GUI.
- [ ] Verify TCP/UDP/IP.
- [ ] Close both GUIs.
- [ ] Verify network remains active.

**Acceptance**

- No terminal networking commands are needed for normal workflow.

## P16-002 — Multi-client single-star acceptance

- [ ] Add supported number of clients.
- [ ] Run concurrent traffic.
- [ ] Exercise leave/rejoin.
- [ ] Verify UI state on each node.

**Acceptance**

- Stable at documented peer-count limit for test duration.

## P16-003 — Routed topology acceptance

- [ ] Build at least two PAN segments.
- [ ] Verify cross-segment traffic.
- [ ] Verify route diagnostics.
- [ ] Record hop performance.

**Acceptance**

- Multi-hop claim is backed by physical-hardware evidence.

## P16-004 — Failure/recovery acceptance

- [ ] range loss.
- [ ] hub shutdown.
- [ ] daemon restart.
- [ ] BlueZ restart.
- [ ] NetworkManager restart.
- [ ] suspend/resume.

**Acceptance**

- Each scenario has expected behavior and recorded result.
- Recoverable scenarios recover without manual route/interface cleanup.

## P16-005 — Resource-use acceptance

- [ ] idle CPU.
- [ ] active CPU.
- [ ] memory.
- [ ] discovery scan impact.
- [ ] sustained transfer impact.

**Acceptance**

- Results are documented and any unacceptable hotspot has a tracked task.

---

# P17 — Documentation and v1 release readiness

## P17-001 — User guide

- [ ] install.
- [ ] create network.
- [ ] join.
- [ ] leave.
- [ ] device status.
- [ ] common recovery steps.

**Acceptance**

- Guide assumes no prior Bluetooth PAN knowledge.

## P17-002 — Administrator/CLI guide

- [ ] commands.
- [ ] JSON output.
- [ ] logs.
- [ ] diagnostics.
- [ ] advanced topology information.

**Acceptance**

- Admin can diagnose a failed link without reading source code.

## P17-003 — Architecture documentation pass

- [ ] Update `SPEC.md` to match implemented reality.
- [ ] Add diagrams.
- [ ] Index ADRs.
- [ ] Document daemon/API boundaries.

**Acceptance**

- No known material architecture drift remains undocumented.

## P17-004 — Troubleshooting guide

Cover at least:

- Bluetooth disabled/missing.
- pairing fails.
- BlueZ unavailable.
- NetworkManager unavailable.
- permission denied.
- peer unreachable.
- address conflict.
- stale/reconnecting state.
- routed peer unavailable.

**Acceptance**

- Common UI error messages point to relevant troubleshooting guidance.

## P17-005 — Define v1 support matrix

- [ ] Debian version.
- [ ] kernel baseline.
- [ ] BlueZ baseline.
- [ ] NetworkManager baseline.
- [ ] tested Chromebook variants.
- [ ] known limitations.

**Acceptance**

- Release does not imply unsupported platforms were tested.

## P17-006 — v1 release gate

- [ ] CI green.
- [ ] hardware acceptance green.
- [ ] security review complete for enabled features.
- [ ] docs current.
- [ ] package clean-install/upgrade/uninstall tests green.
- [ ] no Internet sharing enabled unless P18 is complete.

**Acceptance**

- Release checklist is signed off with exact commit/build identifiers.

---

# P18 — Future Internet gateway support

**This phase is intentionally post-core-product. Do not let it block initial BlueRoute LAN development.**

## P18-001 — Implement external connectivity detector

- [ ] Distinguish link presence from verified Internet reachability.
- [ ] Observe uplink changes.
- [ ] Avoid hard-coding Wi-Fi as the only uplink type.

**Acceptance**

- Daemon can report local external connectivity without offering it to peers.

## P18-002 — Implement gateway opt-in policy

- [ ] User setting: do not share/share.
- [ ] Default off.
- [ ] Persist explicit preference.
- [ ] Surface current state clearly.

**Acceptance**

- Merely having Internet never automatically makes the node a sharing gateway.

## P18-003 — Extend control plane with gateway advertisement

- [ ] gateway availability.
- [ ] willingness to share.
- [ ] preference/metric.
- [ ] freshness/withdrawal.

**Acceptance**

- Gateway advertisements are authenticated as member control messages.

## P18-004 — Implement gateway route selection

- [ ] Default-route candidate model.
- [ ] Path cost to gateway.
- [ ] Preference policy.
- [ ] withdrawal/failure behavior.

**Acceptance**

- Core route tests cover zero, one, and multiple gateways.

## P18-005 — Implement IPv4 forwarding/NAT backend

- [ ] Select NetworkManager shared mode or explicit nftables/forwarding strategy based on topology needs.
- [ ] Configure forwarding.
- [ ] Configure NAT/masquerading.
- [ ] Scope firewall rules to BlueRoute network.
- [ ] Ensure cleanup.

**Acceptance**

- Direct BlueRoute client reaches Internet through gateway.
- Disabling sharing removes gateway-owned NAT/firewall state.

## P18-006 — Implement DNS behavior

- [ ] Define DNS source/forwarding approach.
- [ ] Handle gateway change.
- [ ] Avoid stale DNS configuration on clients.

**Acceptance**

- Client can resolve DNS and access Internet through enabled gateway.

## P18-007 — Prove routed-client Internet access

Test:

```text
client -> routing node -> gateway -> Internet
```

- [ ] Verify multi-hop default route.
- [ ] Verify TCP/UDP/DNS.
- [ ] Measure impact.

**Acceptance**

- Internet access works for a client that is not directly attached to gateway's PAN segment.

## P18-008 — Implement gateway failover

- [ ] Two approved gateways.
- [ ] Select preferred gateway.
- [ ] Detect failure.
- [ ] Withdraw/re-route.
- [ ] Avoid loops.

**Acceptance**

- Hardware test demonstrates bounded failover without manual route changes.

## P18-009 — Add desktop Internet Sharing UI

- [ ] Simple opt-in control.
- [ ] Explain which connection is shared.
- [ ] Show gateway provider on clients.
- [ ] Show unavailable/failover states.
- [ ] Keep NAT/routing jargon in advanced view only.

**Acceptance**

- Non-technical user can enable/disable sharing and understand current state.

## P18-010 — Internet gateway security review

- [ ] Firewall exposure.
- [ ] malicious client behavior.
- [ ] malicious gateway behavior.
- [ ] DNS trust.
- [ ] route injection.
- [ ] accidental uplink exposure.

**Acceptance**

- Threat model is updated before gateway feature is considered production-ready.

---

# P19 — Optional advanced work

These tasks are intentionally not prerequisites for the first useful release.

## P19-001 — IPv6/ULA support

- [ ] Derive/allocate per-network ULA prefix.
- [ ] Configure per-segment prefixes.
- [ ] Route across segments.
- [ ] Evaluate IPv6 Internet gateway behavior separately.

## P19-002 — Invitation code / QR enrollment

- [ ] Design secure invitation representation.
- [ ] Desktop QR display/scan workflow where hardware permits.
- [ ] Expiry/revocation.

## P19-003 — Topology visualization

- [ ] Advanced GUI graph.
- [ ] Direct vs routed links.
- [ ] health/path visualization.
- [ ] Keep optional for normal users.

## P19-004 — Battery-aware topology policy

- [ ] Detect AC/battery where available.
- [ ] Prefer powered nodes for hub roles if useful.
- [ ] Avoid destabilizing topology for minor battery changes.

## P19-005 — Evaluate non-NetworkManager Linux backend

- [ ] Consider systems using systemd-networkd or direct netlink.
- [ ] Keep behind adapter boundary.

## P19-006 — Additional Linux distribution support

- [ ] Define support criteria.
- [ ] Test package/service integration.

## P19-007 — Remote management API

- [ ] Only if a concrete use case exists.
- [ ] Must receive independent authentication/security design.
- [ ] Do not expose local privileged D-Bus API directly over network.

---

# Cross-cutting acceptance checklist

For every feature that manipulates networking:

- [ ] Is the operation idempotent?
- [ ] Does failure preserve enough state for reconciliation?
- [ ] Does cleanup remove only BlueRoute-owned state?
- [ ] Is the behavior testable without hardware where possible?
- [ ] Is hardware evidence recorded where hardware behavior matters?
- [ ] Are errors typed and surfaced in diagnostics?
- [ ] Are secrets absent from normal logs?
- [ ] Can CLI/TUI/desktop consume the same daemon state?
- [ ] Does the change preserve future Internet-gateway separation?
- [ ] Does the default GUI avoid unnecessary networking jargon?

# Initial recommended execution order

The recommended first implementation sequence is:

1. `P0-001` through `P0-005` — establish the project baseline.
2. `P1-001` through `P1-004` — prove PAN and choose Linux ownership boundaries before building abstractions around assumptions.
3. `P2-*` and `P3-*` — build the domain/persistence model.
4. `P4-*` — implement production BlueZ/NetworkManager adapters.
5. `P5-*` — make the daemon/API operational.
6. `P6-*` — deliver a complete managed single-star LAN.
7. `P7-*` and `P8-*` — add authenticated control and routed interconnected stars.
8. `P9-*` — automate topology once routed behavior is proven.
9. `P10-*`, `P11-*`, and `P12-*` — finish CLI/TUI/Tauri experiences on the stable daemon.
10. `P13-*` through `P17-*` — hardening, security, packaging, hardware acceptance, and v1 release.
11. `P18-*` — add Internet gateway capability after the local network is dependable.
12. `P19-*` — optional extensions.

# Definition of “first useful milestone”

The first useful milestone is not the desktop GUI. It is a reliable daemon-managed two-node PAN in which:

- both nodes run Debian on reference hardware;
- one creates a BlueRoute network;
- the other joins through the daemon API;
- IPv4 is configured automatically;
- ordinary TCP and UDP traffic works;
- no manual `bluetoothctl`, `nmcli`, `ip addr`, or `ip route` command is required after installation;
- daemon restart can reconcile the connection;
- CLI diagnostics explain the resulting state.

That milestone validates the foundation before substantial UI work.

# Definition of “v1 local-network complete”

The local-network v1 is complete when:

- the single-star workflow is reliable;
- routed interconnected-star networking has physical hardware acceptance evidence;
- topology recovery behavior is documented and tested;
- daemon, CLI, TUI, and Tauri all use the same API;
- non-technical users can create/join/leave from the desktop app;
- installation and system service behavior are packaged for Debian 13;
- threat model and privilege boundaries have been reviewed;
- reference Chromebook hardware limits are documented;
- Internet sharing remains off/absent unless the separate P18 acceptance criteria are completed.

# BlueRoute Specification

**Project:** BlueRoute  
**Repository:** `ekkus93/blueroute`  
**Document status:** Initial architecture specification  
**Primary platform:** Debian Linux  
**Initial reference hardware:** Dell Chromebook 3100 converted to Debian  

## 1. Purpose

BlueRoute is a friendly Linux application for creating ordinary TCP/IP networks over Bluetooth without requiring Wi-Fi infrastructure. It is intended to make Bluetooth PAN networking usable by non-expert users while still exposing strong diagnostic and automation interfaces for administrators and developers.

BlueRoute uses standard Linux Bluetooth PAN facilities rather than Bluetooth Mesh as the IP transport. Bluetooth links are created with BlueZ/BNEP using PAN roles such as PANU and NAP. Linux IP routing is then used to connect multiple Bluetooth PAN segments when a larger or multi-hop topology is needed.

The project has one shared Rust networking engine and multiple front ends:

- a background daemon that owns network state and keeps the network alive when no UI is open;
- a command-line interface for scripting, administration, and diagnostics;
- a terminal user interface for interactive use without a graphical desktop;
- a Tauri desktop application designed for non-technical users.

A later release may optionally route client traffic to the Internet through a BlueRoute node that has another usable uplink such as Ethernet or Wi-Fi. The initial implementation does not need to provide Internet sharing, but the architecture must not require redesign to add it.

## 2. Product principles

1. **Normal IP networking.** Applications must see ordinary IP connectivity. SSH, HTTP, rsync, NFS, TCP, UDP, and other normal IP software should not need BlueRoute-specific support.
2. **Use Linux networking primitives.** BlueRoute must orchestrate BlueZ, BNEP, NetworkManager, kernel routing, and related system services rather than reimplementing Bluetooth or TCP/IP.
3. **Hide networking jargon from ordinary users.** The desktop UI should speak in terms such as “Create network,” “Join,” “Connected,” and “Internet available,” not PANU, NAP, BNEP, route metrics, NAT, or forwarding tables.
4. **One engine, many interfaces.** CLI, TUI, and Tauri front ends must use the same daemon API and domain model.
5. **Network survives UI exit.** Closing the desktop or TUI application must not tear down an active BlueRoute network.
6. **Recover automatically.** Temporary Bluetooth loss, peer restarts, and topology changes should normally recover without manual reconstruction.
7. **Least privilege.** Front ends run unprivileged. Privileged operations are delegated through established Linux services and narrowly scoped authorization.
8. **Diagnosable.** Users should get simple health status while administrators can obtain detailed state, routes, peer information, logs, and machine-readable output.
9. **Incremental topology complexity.** A reliable single-star PAN is built first. Routed interconnected stars and automatic topology management are layered on top without changing the application-facing model.
10. **Future gateway support.** Internet sharing remains opt-in and is modeled separately from Bluetooth link management.

## 3. Goals

### 3.1 Functional goals

BlueRoute shall eventually support:

- discovering nearby BlueRoute-capable Linux devices;
- pairing/trusting devices through a friendly workflow;
- creating a named BlueRoute network;
- joining an existing BlueRoute network;
- forming Bluetooth PAN links using standard Linux Bluetooth support;
- assigning and managing IP addresses automatically;
- providing normal IPv4 connectivity between directly connected members;
- routing traffic across multiple PAN segments;
- selecting and changing topology without requiring the user to understand node roles;
- reconnecting after transient failures;
- displaying network, node, link, route, and health state;
- CLI automation with stable machine-readable output;
- a Ratatui-based TUI;
- a polished Tauri desktop UI;
- systemd service integration;
- diagnostics suitable for troubleshooting real hardware;
- optional future Internet gateway advertisement, selection, forwarding, DNS, and NAT.

### 3.2 User-experience goals

A non-technical user should be able to perform the primary workflow as follows:

1. Open BlueRoute.
2. Choose **Create a Network** or select a nearby network and choose **Join**.
3. Complete a simple trust/pairing step if required.
4. See a clear “Connected” state.
5. Use ordinary applications over the resulting IP network without additional BlueRoute configuration.

The UI may expose an advanced diagnostics screen, but advanced networking terminology must not be required for normal operation.

## 4. Non-goals

The following are explicitly outside the initial scope:

- implementing the Bluetooth controller, HCI stack, L2CAP, BNEP, TCP, UDP, IPv4, or IPv6 stacks;
- transporting arbitrary IP packets through the Bluetooth Mesh protocol;
- replacing BlueZ or NetworkManager;
- creating a general-purpose Wi-Fi mesh manager;
- promising broadband-like throughput over Bluetooth;
- supporting every Linux distribution in the first release;
- providing Internet sharing in the first proof-of-concept milestone;
- acting as an Internet router without explicit user opt-in;
- exposing raw low-level topology controls as the default desktop experience.

## 5. Terminology

### Node
A computer running the BlueRoute daemon.

### BlueRoute network
A logical trusted group of nodes that may form one or more Bluetooth PAN segments and route IP traffic between members.

### PAN
Bluetooth Personal Area Networking using BNEP.

### PANU
Bluetooth PAN User role. A node in this role attaches to a PAN provider.

### NAP
Bluetooth Network Access Point role. A node in this role accepts PAN clients and anchors a PAN segment.

### PAN segment
One local BNEP/IP segment consisting of a NAP and one or more PANU peers.

### Routed topology
A BlueRoute network containing more than one PAN segment, with one or more nodes forwarding IP traffic between segments.

### Direct peer
A node reachable over a direct Bluetooth PAN link.

### Routed peer
A node reachable through one or more BlueRoute routing nodes.

### Gateway
A BlueRoute node that offers an optional route from the private BlueRoute network to an external network or the Internet.

### Control plane
BlueRoute protocol traffic used for membership, capability, topology, route, and health coordination.

### Data plane
Normal user IP traffic carried across PAN links and routed by Linux.

## 6. High-level architecture

```text
                         Linux system

                 +-------------------------+
                 |   blueroute-daemon      |
                 |       Rust              |
                 |-------------------------|
                 | membership              |
                 | topology                |
                 | addressing              |
                 | route decisions         |
                 | reconnect/recovery      |
                 | health/diagnostics      |
                 +------------+------------+
                              |
                       system D-Bus
                              |
              +---------------+----------------+
              |                                |
          +---v---+                    +-------v--------+
          | BlueZ |                    | NetworkManager |
          +---+---+                    +-------+--------+
              |                                |
        Bluetooth/BNEP                  IP interfaces,
                                         addresses,
                                         routes, DNS,
                                         forwarding

                              ^
                              |
                     BlueRoute local API
                              |
              +---------------+----------------+
              |               |                |
       +------v-----+   +-----v------+   +-----v---------+
       | CLI        |   | TUI        |   | Tauri desktop |
       | blueroute  |   | blueroute- |   | blueroute-    |
       |            |   | tui        |   | desktop       |
       +------------+   +------------+   +---------------+
```

The daemon is authoritative for active BlueRoute state. Front ends are clients of the daemon and must not independently manipulate BlueZ, NetworkManager, or routes.

## 7. Proposed Rust workspace

The exact paths may evolve, but architectural boundaries should remain similar to the following:

```text
blueroute/
├── Cargo.toml
├── docs/
│   ├── SPEC.md
│   └── TODO.md
├── crates/
│   ├── blueroute-core/
│   ├── blueroute-protocol/
│   ├── blueroute-linux/
│   └── blueroute-client/
├── apps/
│   ├── daemon/
│   ├── cli/
│   ├── tui/
│   └── desktop/
│       ├── src-tauri/
│       └── src/
└── tests/
```

### 7.1 `blueroute-core`

Pure or mostly pure domain logic:

- node and network identities;
- capability model;
- topology graph;
- link scoring;
- route planning;
- state machines;
- addressing policy;
- health model;
- configuration schema;
- events and domain errors.

It must not contain shell commands or direct BlueZ/NetworkManager implementation details.

### 7.2 `blueroute-protocol`

Shared serialized types for daemon clients and, where appropriate, inter-node control messages:

- API version;
- commands;
- responses;
- events;
- status snapshots;
- diagnostics types;
- compatibility rules.

Serialization should use a well-supported Rust format with explicit versioning. The exact encoding is an implementation decision; domain compatibility is more important than wire-format novelty.

### 7.3 `blueroute-linux`

Linux-specific adapters:

- BlueZ D-Bus client;
- NetworkManager D-Bus client;
- Bluetooth discovery and pairing integration;
- PANU/NAP lifecycle;
- interface inspection;
- address and route application;
- forwarding/firewall adapters;
- system capability detection;
- Polkit-related integration where needed.

This crate should expose typed Rust traits/interfaces to the core rather than leak D-Bus object paths throughout the codebase.

### 7.4 `blueroute-client`

Reusable local daemon client library used by CLI, TUI, and Tauri backend. It handles:

- connection to daemon API;
- API-version negotiation;
- command/response helpers;
- event subscriptions;
- reconnection to a restarted daemon;
- conversion of daemon state into UI-friendly view models where appropriate.

### 7.5 Applications

- `blueroute-daemon`: long-running system networking service.
- `blueroute`: CLI.
- `blueroute-tui`: Ratatui application.
- `blueroute-desktop`: Tauri desktop application with a Rust Tauri backend and web frontend.

## 8. Daemon model

The daemon shall be a long-running service managed by systemd on the initial platform.

Responsibilities:

- own active BlueRoute configuration and runtime state;
- observe BlueZ and NetworkManager state;
- create and tear down PAN links;
- run topology logic;
- apply local addresses and routes;
- maintain control-plane sessions to peers;
- reconnect links when policy allows;
- expose a stable local API;
- emit state-change events;
- persist durable settings and trust information;
- produce structured diagnostics.

The daemon must not require a graphical session. Active networking must continue after all front ends exit.

## 9. Local daemon API

The preferred local IPC mechanism is D-Bus because BlueRoute is a Linux system networking service and D-Bus provides service discovery, typed method boundaries, signals, permissions, and integration with the rest of the platform.

A service/interface naming scheme should be reserved early, for example:

```text
org.blueroute.Service1
/org/blueroute/Service1
```

The exact names are subject to implementation review.

The local API should include semantic operations rather than low-level network commands. Representative operations include:

- `GetStatus`
- `ListNetworks`
- `CreateNetwork`
- `JoinNetwork`
- `LeaveNetwork`
- `ListNodes`
- `GetNode`
- `SetDeviceName`
- `StartDiscovery`
- `StopDiscovery`
- `TrustPeer`
- `ForgetPeer`
- `GetDiagnostics`
- future `SetInternetSharing`

Representative events include:

- network discovered/lost;
- node discovered/changed;
- node connected/disconnected;
- network joined/left;
- topology changed;
- route changed;
- daemon health changed;
- Internet availability changed;
- authorization required or failed.

All public IPC interfaces must be versioned. Front ends should detect incompatible daemon versions and present a useful error instead of failing unpredictably.

## 10. Bluetooth integration

### 10.1 BlueZ

BlueRoute will use the system BlueZ service through D-Bus for Bluetooth operations. It should not invoke `bluetoothctl` as the production backend. Shell tools may be used during early hardware experiments only.

Required integration areas include:

- adapter enumeration and power state;
- nearby-device discovery;
- device properties;
- pairing and trust;
- connection state;
- PAN network profile access;
- NAP registration or equivalent BlueZ/NetworkManager integration;
- error mapping into BlueRoute domain errors.

### 10.2 BNEP/PAN

The data plane is Bluetooth PAN using BNEP.

Initial proof-of-concept topology:

```text
             NAP node
          /     |     \
       PANU    PANU   PANU
```

This topology is deliberately simple. Once reliable, BlueRoute can build a routed graph from multiple PAN segments.

### 10.3 No Bluetooth Mesh IP tunneling

BlueRoute explicitly does not encapsulate arbitrary TCP/IP packets into Bluetooth Mesh messages. Multi-hop behavior is provided by Linux IP routing between standard PAN links.

## 11. Network topology

### 11.1 Single-star baseline

The first operational milestone is one NAP with one or more PANU clients. All members must be able to use normal IP traffic on the PAN.

### 11.2 Interconnected stars

A larger BlueRoute network may contain multiple PAN segments:

```text
        client A       client B
            \             /
             \           /
                hub 1
                  |
                  | routed PAN link
                  |
                hub 2
              /       \
        client C     client D
```

Traffic from client A to client D is ordinary routed IP traffic:

```text
client A -> hub 1 -> hub 2 -> client D
```

### 11.3 Routing, not giant bridging

The preferred multi-segment design is layer-3 routing rather than attempting to bridge all PAN segments into one large layer-2 broadcast domain. This reduces loop risk, limits broadcast propagation, and gives BlueRoute explicit topology and route control.

### 11.4 Automatic role selection

Ordinary users should not choose PANU/NAP roles. The topology engine eventually selects roles based on:

- which nodes can directly hear/connect to each other;
- measured link quality and stability where available;
- controller connection limits;
- current node degree/load;
- path cost;
- whether a node is battery constrained;
- gateway capability, when Internet sharing is implemented;
- topology stability and avoidance of needless churn.

Manual role forcing may exist only as an advanced diagnostic/development feature.

### 11.5 Topology convergence

Topology changes should be incremental. BlueRoute should prefer keeping a healthy existing topology over continuously rebuilding it for tiny metric improvements.

When a hub disappears, surviving nodes should attempt to establish replacement links and update routes automatically where the physical Bluetooth graph permits it.

## 12. Addressing

### 12.1 IPv4 first

The first production data plane should support IPv4. Each routed PAN segment should use a distinct private subnet selected from a BlueRoute-managed address pool.

The exact default pool must be selected after collision analysis. BlueRoute must detect conflicts with locally connected networks and avoid installing routes that overlap active non-BlueRoute networks.

### 12.2 Stable logical identity vs address

Node identity must not be defined by an IP address. A node keeps a stable BlueRoute identity even if its current PAN segment or address changes.

### 12.3 DHCP vs explicit assignment

The implementation may use NetworkManager shared-mode DHCP for the earliest single-star prototype, but the final addressing design must support deterministic orchestration across multiple routed segments. Address allocation logic belongs behind an abstraction so early DHCP choices do not become protocol assumptions.

### 12.4 IPv6

IPv6 is a planned capability, not an initial blocker. The design should reserve support for a per-network ULA prefix and routing model without requiring the core domain model to be rewritten.

## 13. Routing

The kernel remains responsible for packet forwarding. BlueRoute computes desired route state and applies it through Linux networking adapters.

The core route model must support destinations that include:

- an individual logical node where needed;
- a BlueRoute segment/prefix;
- the entire BlueRoute network where appropriate;
- a future default/Internet route.

Route decisions should account for:

- reachability;
- hop count/path cost;
- link state;
- topology stability;
- route freshness;
- optional gateway preference.

The initial routed implementation may use centrally computed routes if that simplifies correctness. A dynamic routing protocol such as Babel may be evaluated later, but BlueRoute must not depend on choosing a third-party dynamic routing protocol before the underlying PAN behavior is characterized.

## 14. Control plane

BlueRoute requires a control plane separate from user data traffic.

Once nodes have sufficient connectivity, control messages coordinate:

- node identity;
- network identity;
- software/protocol version;
- capabilities;
- neighbor/link observations;
- topology membership;
- route advertisements or assignments;
- health;
- gateway availability in a later release.

Control messages must be bounded, versioned, and validated before use. Malformed or unauthenticated control data must never cause arbitrary system commands to execute.

The transport and authentication mechanism for inter-node control messages should be selected during implementation after the PAN proof of concept. The core protocol must not depend on source IP alone as proof of identity.

## 15. Identity, pairing, and trust

BlueRoute needs two related but distinct notions:

1. Bluetooth device pairing/trust managed through BlueZ.
2. BlueRoute network membership managed by BlueRoute.

A Bluetooth-paired device is not automatically entitled to become a member of every BlueRoute network.

Each installation should have a stable generated node identifier and a user-editable display name.

A BlueRoute network should have a stable network identifier plus a human-friendly name.

The initial membership flow may use an explicit approval step on the creating node. Later versions may add invitation codes or QR-based enrollment. Security-sensitive material must not be displayed in logs by default.

## 16. Security requirements

- Front ends run without root privileges.
- The daemon must use the minimum permissions required for networking operations.
- Prefer BlueZ and NetworkManager D-Bus APIs over spawning privileged shell commands.
- Use Polkit/system D-Bus policy for privileged actions where appropriate.
- Never construct shell commands from peer-provided strings.
- Validate all IPC and network protocol inputs.
- Persist secrets with restrictive filesystem permissions.
- Do not automatically expose an Internet uplink.
- Internet sharing must be explicit, visible, and reversible.
- Peer display names are untrusted presentation data and must not be used as authorization identities.
- Logging must redact secrets and sensitive tokens.
- Destructive operations such as forgetting a network should require clear user intent.

A formal threat model should be created before automatic multi-node trust and Internet gateway features are declared production-ready.

## 17. Privilege model

The desired model is:

```text
unprivileged front ends
        |
        v
BlueRoute system daemon
        |
        +--> BlueZ system D-Bus
        +--> NetworkManager system D-Bus
        +--> narrowly scoped system networking operations
```

The daemon should not run as unrestricted root merely because it is convenient. Implementation work must identify which operations can be delegated through BlueZ/NetworkManager and which, if any, require additional capabilities or helper components.

Authorization failures must be surfaced as user-actionable errors.

## 18. Persistence

Persistent state may include:

- local node identity;
- device display name;
- known BlueRoute networks;
- network membership/trust data;
- remembered peers;
- user preferences;
- advanced policy overrides;
- future gateway preferences.

Transient state such as current RSSI, temporary routes, and live interface names should not be treated as durable identity.

Configuration schemas must be versioned and migrations tested.

## 19. State and lifecycle

Representative daemon/network states include:

- stopped;
- idle;
- discovering;
- joining;
- connected;
- degraded;
- reconnecting;
- leaving;
- error.

The actual implementation may model several orthogonal state machines instead of one giant enum. For example, adapter availability, membership, link state, topology state, and gateway state should not be forced into an invalid combinatorial state model.

Transitions must be idempotent where practical. Repeated “join” or recovery operations must not accumulate duplicate routes or stale interfaces.

## 20. Reliability and recovery

BlueRoute must expect:

- Bluetooth adapters disappearing/reappearing;
- peers moving out of range;
- suspend/resume;
- daemon restarts;
- NetworkManager restarts;
- stale BNEP interfaces;
- failed pairing attempts;
- IP conflicts;
- partial topology partitions;
- hubs losing power;
- multiple peers attempting topology changes at nearly the same time.

Recovery behavior should prefer reconciliation from observed system state rather than assuming every prior command succeeded.

The daemon should periodically or event-driven reconcile desired state against BlueZ, NetworkManager, interfaces, and routes.

## 21. Internet gateway extension

Internet sharing is a planned post-MVP capability.

The architecture must model it now but keep it disabled unless explicitly implemented and enabled.

### 21.1 Gateway model

A node may report capabilities such as:

- external connectivity present;
- connectivity type;
- Internet reachability verified;
- willing to share Internet;
- gateway cost/preference.

“Has Internet” and “is willing to share Internet” are separate states.

### 21.2 Future traffic path

```text
BlueRoute client
      |
      v
one or more routed PAN segments
      |
      v
BlueRoute gateway
      |
      v
Ethernet / Wi-Fi / other uplink
      |
      v
Internet
```

### 21.3 Linux implementation

The gateway adapter may use NetworkManager shared networking where it fits the topology or explicit forwarding/NAT/firewall configuration where required. This decision should be based on actual routed topology requirements rather than hard-coded into the core.

Future gateway support must include:

- IP forwarding;
- NAT/masquerading where required;
- DNS handling;
- default-route advertisement/selection;
- cleanup on disable or failure;
- prevention of forwarding loops;
- firewall policy;
- clear UI indication;
- optional automatic failover to another approved gateway.

### 21.4 Gateway safety

Internet sharing must be off by default in the first release containing the feature. The UI must clearly identify which connection is being shared and which BlueRoute network can use it.

## 22. CLI requirements

The CLI is both an administrator interface and an acceptance-test surface.

Representative commands:

```text
blueroute status
blueroute network list
blueroute network create <name>
blueroute network join <id-or-name>
blueroute network leave
blueroute node list
blueroute node show <id>
blueroute peer trust <id>
blueroute peer forget <id>
blueroute discover
blueroute diagnose
```

The CLI should support structured output, preferably JSON, for read-only status and diagnostic commands.

Exit codes must distinguish success, user/configuration errors, authorization errors, unavailable daemon/system services, and operational failures where practical.

The CLI must call the daemon API rather than implement a second networking stack.

## 23. TUI requirements

The TUI should use Ratatui and the shared client library.

Primary screens:

- overall status;
- nearby/known networks;
- connected nodes;
- node details;
- diagnostics/log view;
- settings.

Keyboard interactions must be discoverable on screen. The TUI must remain useful over a local terminal without a graphical desktop.

## 24. Tauri desktop requirements

The desktop application is the primary non-technical user experience.

### 24.1 Main workflow

The default interface should make these actions prominent:

- Create a Network
- Join
- Leave
- Devices
- Settings
- Diagnose a Problem

### 24.2 Language

Default UI terminology should prefer:

- “network” instead of “PAN segment”;
- “computer/device” instead of “PANU node”;
- “connected through another device” instead of “multi-hop routed peer”;
- “Internet sharing” instead of “NAT/default-route advertisement.”

Technical terms may appear in an advanced diagnostic view.

### 24.3 Status presentation

The application should summarize state in user-centered terms such as:

- Everything looks good
- Connecting…
- Reconnecting…
- Some devices are unreachable
- Bluetooth is turned off
- Permission required
- Internet available
- Internet unavailable

### 24.4 Tauri boundary

The Tauri Rust backend should use `blueroute-client` to talk to the daemon. Browser-side code must not receive unrestricted access to system D-Bus or execute arbitrary system commands.

## 25. Diagnostics and observability

BlueRoute must provide both friendly and detailed diagnostics.

Useful diagnostic data includes:

- BlueRoute version;
- daemon version/API version;
- Linux/kernel version;
- BlueZ availability/version;
- NetworkManager availability/version;
- Bluetooth adapter identity and state;
- current logical network;
- direct peers;
- routed peers;
- BNEP/PAN interfaces;
- assigned addresses;
- routing table entries owned by BlueRoute;
- topology graph;
- recent state transitions;
- authorization failures;
- gateway state when implemented.

Logs should be structured where practical and integrated with journald for the daemon.

A support bundle feature may be added later, but it must redact secrets and private membership credentials.

## 26. Error model

Errors should be typed and categorized. At minimum the domain should distinguish:

- unsupported platform/system configuration;
- missing Bluetooth adapter;
- adapter disabled;
- BlueZ unavailable;
- NetworkManager unavailable;
- peer unavailable;
- pairing rejected/failed;
- authorization denied;
- PAN setup failure;
- address conflict;
- route application failure;
- topology failure;
- protocol incompatibility;
- invalid/corrupt persisted state;
- internal error.

Front ends convert these into audience-appropriate messages. Raw D-Bus errors should remain available in advanced diagnostics but should not be the only user-facing explanation.

## 27. Hardware characterization

Dell Chromebook 3100 is the initial reference platform, but Bluetooth controller behavior must be measured rather than assumed.

Required measurements include:

- controller and driver identification;
- supported Bluetooth features;
- reliable simultaneous PAN connection count;
- connection-establishment behavior;
- stability under sustained TCP traffic;
- throughput and latency for one link;
- throughput and latency under multiple clients;
- routed two-hop behavior;
- CPU use;
- memory use;
- power impact where practical;
- suspend/resume behavior;
- recovery after range loss.

BlueRoute must not encode an assumed maximum number of peers until hardware testing establishes practical limits. Limits should be capability/policy data, not magic constants scattered through the code.

## 28. Testing strategy

### 28.1 Unit tests

Pure core logic should have deterministic unit tests for:

- identifiers and serialization;
- state machines;
- topology graph operations;
- route selection;
- address allocation/conflict avoidance;
- policy decisions;
- health aggregation;
- configuration migrations.

### 28.2 Adapter tests

Linux adapters should use trait boundaries and fakes/mocks for most tests. Tests should verify D-Bus translation, error mapping, idempotency, and reconciliation behavior.

### 28.3 Integration tests

Where feasible, Linux network namespaces and virtual interfaces should test route computation/application independently of Bluetooth hardware.

BlueZ/NetworkManager integration tests should be separated from deterministic core tests so CI does not require physical Bluetooth hardware for every run.

### 28.4 Hardware acceptance tests

A documented hardware test suite must run on at least two Dell Chromebook 3100 units before the first useful release. Routed topology acceptance requires additional nodes.

Hardware tests should record exact software versions and produce reproducible evidence rather than rely on anecdotal success.

### 28.5 UI tests

- CLI command and JSON contract tests;
- TUI state/view tests where practical;
- Tauri unit/component tests;
- end-to-end desktop tests for create/join/leave/error flows once the backend is stable.

## 29. CI and quality gates

The repository should eventually enforce:

- `cargo fmt --check`;
- `cargo clippy` with agreed lint policy;
- Rust unit/integration tests;
- frontend formatting/linting/type checks;
- Tauri/frontend tests;
- documentation checks where practical;
- dependency/security review tooling as appropriate;
- reproducible packaging checks for supported Debian targets.

Hardware acceptance is a separate gate and must not be falsely represented as covered by ordinary CI.

## 30. Packaging and installation

Initial packaging target is Debian.

The installed system may include:

- `blueroute-daemon` binary;
- CLI binary;
- TUI binary;
- desktop application;
- systemd unit;
- D-Bus service/policy files;
- Polkit rules/actions if required;
- desktop launcher/icon metadata;
- configuration directories;
- man pages or generated CLI documentation.

Uninstall must not leave stale BlueRoute routes, firewall rules, or automatically created network profiles active.

## 31. Compatibility and dependencies

The initial supported environment should explicitly document minimum tested versions for:

- Debian;
- Linux kernel;
- BlueZ;
- NetworkManager;
- systemd;
- Rust toolchain for source builds;
- WebKit/other Tauri runtime dependencies.

The first development target is Debian 13 on Dell Chromebook 3100. Portability to other modern Linux distributions is desirable but secondary to getting the reference platform correct.

## 32. Performance expectations

BlueRoute is intended for management, file transfer, SSH, application control, local services, and similar IP workloads where Bluetooth performance is acceptable. It is not intended to claim Wi-Fi-equivalent bandwidth.

No hard throughput target should be published until measurements are collected on the reference hardware. The project should instead establish baseline metrics and regression thresholds from hardware characterization.

Topology algorithms must account for the fact that every additional wireless hop consumes capacity and increases latency.

## 33. Resource use

The daemon should be lightweight enough for Chromebook-class hardware. Idle CPU use should be near-zero except for necessary system events/timers, and discovery scans should not run continuously without policy justification.

Memory, wakeup frequency, and scan behavior should be measured during hardware acceptance.

## 34. Privacy

BlueRoute should minimize broadcast metadata. Device/network names shown to other peers should be user-controlled. Diagnostic exports must avoid secrets and should warn before including identifiers that may be sensitive.

Internet connectivity checks, when later implemented, should be documented and configurable if they contact external endpoints.

## 35. Initial release layering

### Layer A: PAN proof of concept

- two Debian nodes;
- one NAP, one PANU;
- automatic or scripted setup through production adapters;
- IPv4 connectivity;
- TCP traffic succeeds.

### Layer B: Managed single-star network

- daemon;
- persistent node/network identity;
- multiple clients where hardware permits;
- automatic address configuration;
- create/join/leave;
- CLI diagnostics;
- reconnection.

### Layer C: Routed interconnected stars

- multiple PAN segments;
- forwarding;
- topology graph;
- routes computed/applied automatically;
- multi-hop TCP/IP;
- failure recovery.

### Layer D: Friendly applications

- TUI;
- polished Tauri desktop workflow;
- packaging and installation.

### Layer E: Internet gateway

- explicit gateway enablement;
- forwarding/NAT/DNS;
- gateway advertisements;
- default-route selection;
- optional failover.

## 36. Acceptance definition for core product

BlueRoute reaches its initial core-product objective when, on supported Debian hardware:

1. A non-technical user can create a BlueRoute network from the desktop application.
2. Another device can discover and join it through the desktop application.
3. The devices obtain working IP connectivity without manual `ip`, `nmcli`, `bluetoothctl`, or route commands.
4. Ordinary TCP and UDP applications work across the network.
5. Closing the UI does not stop networking.
6. Restarting a peer or temporarily losing Bluetooth connectivity results in understandable state and automatic recovery when possible.
7. CLI and TUI show the same underlying daemon state as the desktop application.
8. Diagnostics can explain the active interfaces, addresses, peers, and routes.
9. Multi-segment routing works before BlueRoute claims automatic routed topology support.
10. Internet sharing, if not yet implemented, remains cleanly absent rather than partially enabled.

## 37. Open engineering questions

These questions should be answered by prototypes and recorded decisions rather than guessed up front:

- What Bluetooth chipset/driver variants exist across Dell Chromebook 3100 units?
- How many simultaneous BNEP/PAN links are reliable on the reference hardware?
- Should NetworkManager own all PAN profiles, or should some NAP/PAN operations use BlueZ directly with NetworkManager managing only IP state?
- What is the best pre-PAN mechanism for positively identifying a discovered device as BlueRoute-capable?
- What enrollment method best balances friendliness and trust security?
- What private IPv4 pool minimizes collisions while remaining easy to diagnose?
- What inter-node control transport best fits connected PAN segments?
- Should initial routed topology be centrally coordinated, distributed, or hybrid?
- Is an existing routing protocol such as Babel advantageous after hardware constraints are understood?
- Which D-Bus/Polkit permissions are actually required on Debian 13?
- How should suspend/resume affect NAP responsibilities?
- What topology metrics are reliable from BlueZ/controller data on this hardware?
- What gateway/NAT approach works best once routed multi-segment networks exist?

## 38. Architectural invariants

The following are considered project-level invariants unless this specification is deliberately revised:

1. BlueRoute carries normal IP over standard Bluetooth PAN/BNEP links.
2. Bluetooth Mesh is not the IP transport.
3. Multi-hop connectivity is solved at the IP routing/topology layer.
4. The daemon is authoritative for network state.
5. CLI, TUI, and desktop front ends share the daemon API rather than duplicate networking logic.
6. Front ends do not require root privileges.
7. The core domain model is separated from Linux/BlueZ/NetworkManager adapters.
8. Internet sharing is a distinct, opt-in gateway feature.
9. The UI hides low-level networking concepts from ordinary users.
10. Hardware-dependent connection limits are measured and represented as capabilities/policy, not assumed.

## 39. Documentation requirements

As implementation progresses, the project should add and maintain:

- architecture decision records for material design choices;
- developer setup instructions;
- protocol/API documentation;
- daemon D-Bus API documentation;
- hardware test procedures and results;
- troubleshooting guide;
- packaging/install guide;
- user guide for desktop/TUI/CLI;
- security/threat-model documentation before production trust/gateway features.

This document defines the intended architecture and product direction. `docs/TODO.md` decomposes it into implementation tasks and acceptance gates.

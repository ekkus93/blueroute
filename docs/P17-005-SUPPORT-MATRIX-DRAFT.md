# BlueRoute v1 support matrix — evidence-gated draft

**Status (2026-11-04): DRAFT / NOT A v1 support claim.** This matrix distinguishes demonstrated component behavior from a qualified end-to-end release. No general-availability platform, minimum kernel, controller capacity, multi-hop configuration, or production join has been certified. See [TODO](TODO.md), [hardware acceptance backlog](HARDWARE_ACCEPTANCE_BACKLOG.md), and [v1 release gate](TODO.md#p17-006--v1-release-gate).

## Support classifications

| Classification | Meaning |
| --- | --- |
| **Observed component behavior** | A named configuration has evidence for the *specific* operation tested; it is not a complete BlueRoute product qualification. |
| **Software simulated** | Deterministic Rust, fake-backend, real D-Bus broker, localhost TCP/TLS or isolated dpkg checks; cannot establish Bluetooth radio behavior. |
| **Development target** | A platform used for implementation/builds, not a validated minimum or compatibility guarantee. |
| **Not qualified** | No sufficient linked acceptance evidence. Treat as unsupported for release claims, even if the underlying technology should work. |

## Current platform and feature evidence

| Dimension | Current evidence | Classification | Missing for v1 |
| --- | --- | --- | --- |
| Linux distribution | Debian 13 appears in the existing P4/P5/P6 hardware evidence and development-package instructions. | Observed component behavior / development target | Clean installation, full authenticated join, recovery, upgrade/removal and end-to-end qualification on identified systems. |
| Other Linux distributions | No completed distribution-specific end-to-end qualification recorded in the authoritative TODO. | Not qualified | Record distribution, package integration, systemd, PolicyKit, D-Bus, BlueZ and network backend compatibility. |
| Kernel | Linux kernel networking and Bluetooth interfaces exercised by P4 tests; no version-independent minimum established. | Observed component behavior | Record exact kernel releases for each physical test and determine a justified minimum from evidence. |
| Bluetooth stack | BlueZ discovery, pairing, NAP and PANU operations exercised in P4 hardware tests. | Observed component behavior | Record exact BlueZ versions per rig; verify authenticated control and production join on physical links. |
| Network backend | NetworkManager D-Bus, address, bridge, route and forwarding operations exercised on Debian 13. | Observed component behavior | Record exact NetworkManager versions; verify owned-resource recovery across service restart and clean uninstall. |
| systemd / D-Bus / PolicyKit | Daemon activation and authorization were exercised on Debian; isolated broker tests run in CI. | Observed component behavior / software simulated | Validate installation, upgrades, service restarts and policy behavior on clean target systems. |
| Controller and computer model | Existing P4 evidence includes a Debian Chromebook development rig; the Dell Chromebook 3100 family is a fixture, **not** the required product model. | Observed component behavior only | Per-test adapter manufacturer/model, firmware, chipset, driver, kernel, supported NAP/PANU roles and independent second controller class. |
| NAP host role | P4/P6 host operations have physical evidence. | Observed component behavior | Production authenticated join, multiple-client concurrency, disconnect/recovery and bounded role capacity. |
| PANU client role | P4 PANU connectivity has physical evidence. | Observed component behavior | Production outbound PANU session lifecycle and independently verified peer-key enrollment. |
| Client-only controllers | Capability and conservative policy reporting exist; no general hardware support claim. | Software simulated / not qualified | P16-007 real client-only device qualification. |
| Peer count / throughput | No general peer ceiling, sustained throughput or power budget established. | Not qualified | Record per-controller concurrent peer and traffic results without extrapolation. |
| Multi-hop and routed stars | Topology/routing software is partial; no release-grade physical routing proof. | Not qualified | P8/P9/P16 routed traffic, failure and topology acceptance. |
| Internet sharing | Not enabled for production; P18 is future work. | Not qualified | Separate opt-in, route/NAT/DNS security and physical acceptance. |

## Mandatory evidence record for each tested configuration

For every future compatibility row, capture: test date; source SHA and package hash; distribution and version; exact kernel; BlueZ, NetworkManager, systemd and PolicyKit versions; computer model; Bluetooth controller USB/PCI ID, chipset, firmware and driver; supported and exercised NAP/PANU roles; radio/path isolation; exact topology and peer count; commands and raw logs; successful and failed assertions; recovery and teardown behavior; and links to versioned hardware evidence. State whether the observation is **software-only**, **component hardware**, or **complete end-to-end**. Do not infer capability from a model name alone.

## Release decision

**No v1-supported configuration is currently declared by this draft.** The final matrix must be generated from P16 acceptance records, and P17-006 must remain open until those records, security review, complete packaging checks and production authenticated joining are verified. Discovery metadata, trust flags, a healthy local daemon, simulated TCP/TLS and package extraction are not substitutes for a physical authenticated PAN data path.

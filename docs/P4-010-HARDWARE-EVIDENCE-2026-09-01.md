# P4-010 Hardware Acceptance — System Capability Report

**Task:** P4-010 — Implement system capability report  
**Test date:** 2026-09-01 (local PDT)  
**Host:** `debiancb1`  
**Candidate commit:** `90244456658e61d983cd5ca3c8452fb4a49dc4e8`  
**Probe:** `blueroute-linux/examples/system_capability_probe.rs`

## Purpose

Validate that the read-only Rust system capability probe reports enough live Linux runtime information to explain whether the host is fully supported, client-only, degraded, or unsupported, without mutating Bluetooth, NetworkManager, route, forwarding, firewall, or NAT state.

## Invocation

```bash
cd ~/work/blueroute
git fetch origin
git switch P4-010_system_capability_report
git pull --ff-only origin P4-010_system_capability_report
git log -1 --oneline

cargo build -p blueroute-linux \
  --example system_capability_probe \
  --locked

./target/debug/examples/system_capability_probe
```

The checked-out candidate was:

```text
9024445 (HEAD -> P4-010_system_capability_report, origin/P4-010_system_capability_report) fix: satisfy P4-010 Clippy contracts
```

## Observed report

```text
support: FullySupported
BlueZ: available=true version=5.82
network backend: Some(NetworkManager) version=1.52.1
kernel: 6.12.86+deb13-amd64
controllers: 1
  /org/bluez/hci0 powered=true address=unknown driver=btusb
PANU available: true
NAP available: true
IPv4 forwarding: available=true enabled=false
peer ceiling: practical=4 configured=none effective=4
runtime prerequisites:
  bluetooth-sysfs available=true detail=/sys/class/bluetooth
  bnep available=true detail=/sys/module/bnep
  ipv4-forwarding-control available=true detail=/proc/sys/net/ipv4/ip_forward
  bluez-system-service available=true detail=org.bluez
  networkmanager-system-service available=true detail=org.freedesktop.NetworkManager
diagnostics:
  Info [summary] system satisfies current BlueRoute PANU, NAP, network-backend, and forwarding prerequisites
  Info [panu] PANU prerequisites are present; remote peer compatibility is evaluated when connecting
```

## Acceptance assessment

The report correctly classified the tested Debian host as `FullySupported` and explained that classification using independent observations for:

- BlueZ service availability and version (`5.82`);
- NetworkManager availability and version (`1.52.1`);
- the powered BlueZ adapter `/org/bluez/hci0` and Linux `btusb` driver;
- PANU prerequisites;
- observable NAP capability;
- Linux BNEP runtime support;
- IPv4 forwarding control availability and current disabled state;
- the conservative practical peer ceiling and effective configured ceiling;
- kernel/runtime prerequisites.

The controller address was reported as `unknown` because the current read-only sysfs enrichment path did not expose it on this host. That does not affect P4-010 acceptance: the capability decision does not depend on the Bluetooth address, controller identification remains available through the BlueZ object path, and BlueRoute authorization identity is explicitly separate from display/controller addressing.

The probe was run without `sudo` and performed no configuration changes. This is consistent with the P4-010 requirement that diagnostics be observational rather than silently repairing or mutating host state.

## CI evidence

GitHub Actions run `33576973532` passed for candidate `90244456658e61d983cd5ca3c8452fb4a49dc4e8`, including:

- `cargo fmt --all -- --check`;
- `cargo check --workspace --locked`;
- `cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `cargo test --workspace --locked`.

## Result

**PASS.** P4-010 acceptance is satisfied on the supported Debian hardware baseline. The probe explains the host's support state and exposes the required runtime/capability evidence without requiring shell-tool parsing for production networking operations or mutating the system.
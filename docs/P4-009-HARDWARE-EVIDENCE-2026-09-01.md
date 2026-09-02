# P4-009 — IPv4 forwarding hardware evidence

**Date:** 2026-09-01  
**Task:** P4-009 — Implement forwarding adapter  
**Candidate revision:** `ebe460d426893458fd27e8e8a6a7ffef88508284` (`style: format P4-009 forwarding probe`)

## Acceptance scope

This acceptance validates the Linux implementation of `IpNetworkBackend::set_ipv4_forwarding(bool)` against the live kernel. P4-009 controls only node-global IPv4 forwarding (`net.ipv4.ip_forward`). NAT, masquerading, firewall policy, NetworkManager shared mode, gateway selection, and default-route management are outside this task.

The primary acceptance host exercises the actual `0 -> 1 -> 0` kernel transition. A supplementary second-host run starts with forwarding already enabled and verifies that BlueRoute does not disable pre-existing/foreign forwarding state when its lease is released.

## Primary host: `debiancb1`

- Computer: `debiancb1`
- Linux distribution: Debian GNU/Linux 13 (trixie)
- Kernel: `6.12.86+deb13-amd64`
- BlueZ: `5.82`
- Network backend: NetworkManager `1.52.1`
- Bluetooth adapter: not exercised by P4-009; this task is a node-global kernel/network-backend primitive rather than a Bluetooth-link lifecycle test
- Git revision: `ebe460d426893458fd27e8e8a6a7ffef88508284`

### Pre-test state

The host was on the exact candidate revision and had no BlueRoute forwarding lease:

```text
Current IPv4 forwarding: 0
default via 100.64.64.1 dev wlp1s0 proto dhcp src 100.64.67.42 metric 600
No pre-existing BlueRoute forwarding lease
```

This establishes a clean disabled baseline and a pre-existing default route to preserve.

### Rust probe result

The already-built probe was run with elevation; Cargo itself was not run as root:

```text
NetworkManager version: 1.52.1
baseline IPv4 forwarding: 0
forwarding enabled: kernel=1 lease=schema=1;baseline=0
forwarding lease rediscovered by fresh backend connection
holding IPv4 forwarding for 30s; independently inspect /proc/sys/net/ipv4/ip_forward and /run/blueroute/ipv4-forwarding-v1.state
forwarding restored: baseline=0 repeated-release=succeeded lease=absent
P4-009 IPv4 forwarding probe PASS
```

The fresh-backend step proves reconciliation does not depend on process-local state.

### Independent live-kernel inspection during hold

A second terminal observed the actual kernel and lease state while the Rust probe was holding forwarding enabled:

```text
Kernel forwarding during hold: 1
BlueRoute lease:
schema=1
baseline=0
Lease permissions:
600 root:root /run/blueroute/ipv4-forwarding-v1.state
Default route:
default via 100.64.64.1 dev wlp1s0 proto dhcp src 100.64.67.42 metric 600
```

This proves:

- the kernel forwarding bit actually changed from `0` to `1`;
- BlueRoute recorded the exact disabled baseline;
- the runtime lease was owner-restricted (`0600`, `root:root`);
- the existing default route remained unchanged while forwarding was enabled.

### Post-release inspection

After the probe completed its repeated release path:

```text
Kernel forwarding after release: 0
BlueRoute lease removed
default via 100.64.64.1 dev wlp1s0 proto dhcp src 100.64.67.42 metric 600
```

The live kernel therefore completed the required `0 -> 1 -> 0` lifecycle, the BlueRoute lease was removed, repeated release succeeded, and the pre-existing default route was preserved.

## Supplementary host: `arisu`

A second run validated the complementary ownership case where IPv4 forwarding was already enabled before BlueRoute acquired a lease.

- Computer: `arisu`
- Network backend observed by the probe: NetworkManager `1.36.6`
- Git revision: `ebe460d426893458fd27e8e8a6a7ffef88508284`
- Pre-test forwarding: `1`
- Pre-test BlueRoute lease: absent
- Pre-existing default route: `default via 100.64.64.1 dev wlo1 proto dhcp metric 600`

Probe output:

```text
NetworkManager version: 1.36.6
baseline IPv4 forwarding: 1
forwarding enabled: kernel=1 lease=schema=1;baseline=1
forwarding lease rediscovered by fresh backend connection
forwarding restored: baseline=1 repeated-release=succeeded lease=absent
P4-009 IPv4 forwarding probe PASS
```

Post-run checks showed:

```text
Kernel forwarding after release: 1
BlueRoute lease removed
default via 100.64.64.1 dev wlo1 proto dhcp metric 600
```

This supplementary run proves that release does not blindly write `0` when forwarding pre-dated BlueRoute. It is ownership evidence, not a broad platform-support claim; the primary fully inventoried acceptance host remains `debiancb1`.

## CI evidence

Before hardware acceptance, CI run `33555579064` passed on the exact candidate revision, including:

- `cargo fmt --all -- --check`;
- `cargo check --workspace --locked`;
- `cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `cargo test --workspace --locked`.

## Acceptance conclusion

**PASS.** P4-009 can enable kernel IPv4 forwarding for routed topology without coupling the operation to Internet NAT. It records and restores the prior node-global forwarding state through a restrictive boot-local BlueRoute lease, reconciles that lease through a fresh backend instance, supports repeated idempotent enable/release, preserves pre-existing forwarding owned by another actor, removes its lease after successful release, and leaves the tested pre-existing default routes untouched.

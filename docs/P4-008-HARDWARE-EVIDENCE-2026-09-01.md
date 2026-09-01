# P4-008 hardware evidence — 2026-09-01

## Scope

This record closes the physical acceptance for **P4-008 — Implement route adapter**. The acceptance exercised the Rust `NetworkManagerBackend` on a real Debian host and independently inspected the Linux kernel routing table while the BlueRoute-owned route was active and after cleanup.

The probe used NetworkManager's system D-Bus API for all mutations. `ip` was used only as an independent read-only acceptance oracle for live kernel state.

## Tested revision

- Repository: `ekkus93/blueroute`
- Branch: `P4-008_route_adapter`
- Accepted revision: `c800c08e7e1e8cd3e8bfff4a3e8a8e304137f4c4` (`fix: format P4-008 route probe`)
- PR: #23
- Pre-hardware CI: GitHub Actions run `33548584153` passed rustfmt, workspace check, Clippy with `-D warnings`, and the locked workspace test suite.

## Test host and software

- Host: `debiancb1`
- Distribution: Debian GNU/Linux 13 (trixie)
- Kernel: `6.12.86+deb13-amd64`
- BlueZ: `5.82`
- NetworkManager: `1.52.1`
- Bluetooth adapter: `/org/bluez/hci0`, address `F4:D1:08:70:B7:86`; Bluetooth was not part of the P4-008 route-only data path.

## Probe configuration

The accepted run used:

```text
bridge/interface: br-blue-rt
BlueRoute-owned address: 10.254.90.1/30
route destination: 10.254.91.0/24
next hop: 10.254.90.2
initial metric: 177
final reconciled metric: 77
hold: 90 seconds
```

The probe was built as the normal user and only the already-built binary was elevated for NetworkManager mutation authorization:

```bash
cargo build -p blueroute-linux \
  --example networkmanager_route_probe \
  --locked

sudo ./target/debug/examples/networkmanager_route_probe br-blue-rt 90
```

## Baseline preservation target

Before starting the probe, the host's existing default route was:

```text
default via 100.64.64.1 dev wlp1s0 proto dhcp src 100.64.67.42 metric 600
```

The Rust probe also recorded:

```text
baseline: connections=15 foreign-profiles=15
```

## Rust route lifecycle evidence

The live probe reported successful profile/address setup, add/update/idempotent ensure, fresh-backend rediscovery, ownership rejection, and safe wrong-owner cleanup:

```text
NetworkManager version: 1.52.1
baseline: connections=15 foreign-profiles=15
route test interface ready: interface=br-blue-rt profile=/org/freedesktop/NetworkManager/Settings/19 address=10.254.90.1/30
route ready: destination=10.254.91.0/24 via=10.254.90.2 metric=77 repeated-ensure=single-route update-from-metric=177
route rediscovered and reconciled after fresh backend connection
cross-owner route takeover rejected: cannot add a route without an existing BlueRoute-owned NetworkManager profile
cross-owner route rejection left no leaked state
wrong-owner route cleanup was a safe no-op
foreign NetworkManager profiles preserved before hold
holding configured route for 90s; verify in another terminal with: ip -4 route show 10.254.91.0/24
```

This proves that an initial metric-177 route was reconciled to metric 77 for the same destination instead of accumulating a duplicate, and that a fresh `NetworkManagerBackend` rediscovered durable route state rather than relying on in-memory success.

## Independent live-kernel inspection

During the 90-second hold, the bridge carried the expected BlueRoute-owned address:

```text
8: br-blue-rt: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 qdisc noqueue state DOWN group default qlen 1000
    inet 10.254.90.1/30 brd 10.254.90.3 scope global noprefixroute br-blue-rt
```

The kernel route was present with the exact destination, next hop, interface, and final metric:

```text
10.254.91.0/24 via 10.254.90.2 dev br-blue-rt proto static metric 77 linkdown
```

`ip route get` selected that route and the expected source address:

```text
10.254.91.1 via 10.254.90.2 dev br-blue-rt src 10.254.90.1 uid 1002
    cache
```

The `linkdown` annotation is expected for this isolated route test because no carrier-producing PAN/BNEP member was attached to the bridge. It does not indicate route-installation failure.

The pre-existing default route remained unchanged during the probe:

```text
default via 100.64.64.1 dev wlp1s0 proto dhcp src 100.64.67.42 metric 600
```

## Teardown and preservation evidence

The probe exited naturally with:

```text
route removed; repeated remove succeeded
bridge/profile removed; repeated cleanup succeeded
foreign NetworkManager profiles preserved after cleanup
P4-008 NetworkManager route probe PASS
```

Independent post-probe checks then showed:

- `ip -4 route show 10.254.91.0/24` returned no route;
- `ip link show br-blue-rt` returned `Device "br-blue-rt" does not exist.`;
- the original default route was still present and unchanged:

```text
default via 100.64.64.1 dev wlp1s0 proto dhcp src 100.64.67.42 metric 600
```

## Acceptance result

**PASS.** P4-008 demonstrated on real hardware that the Rust NetworkManager route adapter can:

- inspect BlueRoute-owned configured routes;
- add a route;
- reconcile an existing destination from metric 177 to metric 77 without duplicate accumulation;
- repeat ensure idempotently;
- rediscover and reconcile the route through a fresh backend connection;
- reject cross-owner mutation without leaking state;
- make wrong-owner removal a safe no-op;
- install the intended route into the live Linux kernel;
- remove the route idempotently;
- remove only the BlueRoute-owned temporary interface/profile;
- preserve all 15 baseline foreign NetworkManager profiles;
- preserve the host's pre-existing default route throughout setup and teardown.

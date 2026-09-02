# P4-009 — IPv4 forwarding adapter

P4-009 implements the node-global IPv4 forwarding primitive behind the existing backend-neutral `IpNetworkBackend`. It enables routed BlueRoute topologies to request kernel forwarding without coupling local routing to Internet sharing, NAT, firewall policy, or NetworkManager shared mode.

## Scope

P4-009 implements `IpNetworkBackend::set_ipv4_forwarding(bool)` for the Linux NetworkManager backend.

The adapter:

- controls Linux `net.ipv4.ip_forward` through `/proc/sys/net/ipv4/ip_forward`;
- makes repeated enable/release calls idempotent;
- records the pre-existing node-global forwarding value before BlueRoute changes it;
- restores only state for which BlueRoute holds a runtime lease;
- preserves forwarding that was already enabled before BlueRoute acquired the lease;
- allows a fresh backend instance to rediscover the lease after a backend/daemon reconnect within the same boot;
- fails closed on malformed lease state or unexpected kernel values.

The topology/orchestration layer remains responsible for calling `set_ipv4_forwarding(true)` only on nodes that currently need to route between BlueRoute segments. The adapter is deliberately a primitive, not a topology policy engine.

## Why forwarding needs explicit ownership

`net.ipv4.ip_forward` is a **node-global** kernel setting. It is not scoped to one NetworkManager connection or one BlueRoute `NetworkId`. Blind cleanup such as always writing `0` when BlueRoute stops would be unsafe because an administrator or another service may have enabled forwarding independently.

BlueRoute therefore uses a boot-local runtime lease:

```text
/run/blueroute/ipv4-forwarding-v1.state
```

The current schema is intentionally small:

```text
schema=1
baseline=0
```

or:

```text
schema=1
baseline=1
```

The lease file is created atomically with mode `0600`. `/run` is volatile across reboot, which matches the lifetime of the kernel forwarding state being managed.

## Enable semantics

When forwarding is requested:

1. read `/proc/sys/net/ipv4/ip_forward` and require exactly `0` or `1`;
2. read an existing BlueRoute lease or atomically create one recording the current baseline;
3. if forwarding is disabled, write `1`;
4. re-read the kernel value and require it to be enabled;
5. leave the lease in place until BlueRoute releases forwarding.

Repeated enable calls do not replace the recorded baseline. A newly constructed `NetworkManagerBackend` can read the same runtime lease and reconcile the already-enabled state without relying on process memory.

## Release semantics

When forwarding is released:

- if no BlueRoute lease exists, the call is a no-op; the current global forwarding value is treated as foreign state;
- if the recorded baseline was `0` and forwarding is still `1`, BlueRoute restores `0` and verifies it;
- if the recorded baseline was `1`, BlueRoute leaves forwarding enabled;
- if another actor disabled forwarding while BlueRoute held the lease, release does **not** write `1` and undo that newer external decision;
- after successful release, the lease is removed;
- repeated release is therefore safe.

Malformed or unsupported lease contents fail closed rather than guessing ownership.

## Failure and rollback behavior

If BlueRoute creates a new lease but cannot enable kernel forwarding, it removes the lease before returning the error. If both the primary operation and lease rollback fail, the returned diagnostic preserves both failures so the caller is not told cleanup succeeded when it did not.

Lease creation uses `create_new` semantics. If another BlueRoute caller wins a concurrent creation race, the existing lease must parse correctly and is retained; the losing caller never treats that foreign creation as its own rollback target.

## NAT/firewall boundary

P4-009 does **not**:

- configure NAT or masquerading;
- call `nft`, `iptables`, or firewall-management APIs;
- change NetworkManager `ipv4.method=shared`;
- add a default route;
- advertise or select an Internet gateway.

Those concerns remain separate for the later gateway phase. P4-009 only provides ordinary kernel forwarding needed between routed BlueRoute segments.

## Hardware acceptance probe

The probe is:

```text
crates/blueroute-linux/examples/ipv4_forwarding_probe.rs
```

The probe performs all forwarding mutations through `IpNetworkBackend::set_ipv4_forwarding`. It uses direct reads of procfs and the lease file only as independent acceptance oracles.

The probe:

1. repeatedly releases any stale BlueRoute test lease from an interrupted prior run;
2. records the resulting pre-test forwarding baseline;
3. enables forwarding twice and requires kernel value `1`;
4. requires a valid BlueRoute runtime lease;
5. constructs a fresh `NetworkManagerBackend`, enables again, and requires forwarding to remain enabled;
6. holds the state for independent inspection;
7. releases forwarding twice through the fresh backend;
8. requires the exact pre-test baseline to be restored and the lease to be absent.

### Build and run

On the Debian hardware test host, build as the ordinary user:

```bash
cd ~/work/blueroute
git fetch origin
git switch P4-009_forwarding_adapter
git pull --ff-only origin P4-009_forwarding_adapter
git log -1 --oneline

cargo build -p blueroute-linux \
  --example ipv4_forwarding_probe \
  --locked
```

Writing the kernel forwarding control and `/run/blueroute` normally requires privilege. Run only the already-built binary with elevation:

```bash
sudo ./target/debug/examples/ipv4_forwarding_probe 90
```

Do **not** run Cargo as root.

### Independent inspection during hold

In another terminal:

```bash
cat /proc/sys/net/ipv4/ip_forward
sudo cat /run/blueroute/ipv4-forwarding-v1.state
sudo stat -c '%a %U:%G %n' /run/blueroute/ipv4-forwarding-v1.state
ip -4 route show default
```

Acceptance requires:

- kernel forwarding value `1` during the hold;
- a schema-1 BlueRoute lease with the actual pre-test baseline;
- lease mode `600`;
- no unexpected change to the pre-existing default route.

After the probe exits naturally:

```bash
cat /proc/sys/net/ipv4/ip_forward
test ! -e /run/blueroute/ipv4-forwarding-v1.state && echo lease-absent
ip -4 route show default
```

The final forwarding value must equal the baseline printed by the probe, the lease must be absent, and the default route must remain unchanged. A baseline of `1` is valid and must remain `1`; acceptance must never assume the host originally had forwarding disabled.

## Acceptance status

**Complete.** Hardware acceptance is recorded in `P4-009-HARDWARE-EVIDENCE-2026-09-01.md`.

On `debiancb1`, the exact candidate revision performed the live kernel `0 -> 1 -> 0` transition, recorded `baseline=0` in a `0600` root-owned runtime lease, rediscovered the lease through a fresh backend instance, completed repeated idempotent release, removed the lease, and preserved the pre-existing default route. A supplementary `arisu` run started with forwarding already enabled and proved release preserves that pre-existing/foreign state (`1 -> 1`) rather than blindly disabling it.

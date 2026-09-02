# P4-010 — System capability report

P4-010 adds a read-only Linux capability probe that explains whether the current machine is suitable for BlueRoute and why.

## Scope

`SystemCapabilityProbe` gathers runtime evidence without changing Bluetooth, NetworkManager, routes, forwarding, firewall, or persistent configuration state. It reports:

- BlueZ service availability and `bluetoothd` version when discoverable;
- the active NetworkManager backend and version;
- Bluetooth controller object paths, power state, address, and kernel driver where sysfs exposes them;
- PANU prerequisites and local NAP server capability where BlueZ exposes `org.bluez.NetworkServer1`;
- kernel IPv4 forwarding control availability and current value without mutating it;
- a conservative practical active-peer ceiling plus any configured `max_active_links` override;
- kernel release and required runtime prerequisites such as Bluetooth sysfs and BNEP;
- structured diagnostics and an aggregate support classification.

The probe deliberately does not claim that a particular remote peer will accept PANU connections. PANU availability means the local BlueZ/BNEP/controller prerequisites are present; remote compatibility is evaluated when a connection is attempted.

## Support classification

The report uses four explicit states:

- `FullySupported` — current NetworkManager, BlueZ, powered Bluetooth, BNEP, PANU, NAP, and forwarding prerequisites are present.
- `ClientOnly` — PANU prerequisites are present but local NAP hosting is not available.
- `Degraded` — the system has partial support, such as an adapter that is present but powered off, an indeterminate NAP observation, or missing forwarding support on a would-be routing node.
- `Unsupported` — a required service/runtime prerequisite is missing, including BlueZ, the selected network backend, a Bluetooth adapter, or BNEP.

A client-only system is intentionally not labeled unhealthy merely because it cannot host NAP.

## BlueZ version discovery

BlueZ does not expose the daemon version through the current BlueZ object model used by BlueRoute. The capability probe therefore attempts the daemon's documented `--version` output at the known Debian/Linux locations `/usr/libexec/bluetooth/bluetoothd`, `/usr/lib/bluetooth/bluetoothd`, `/usr/sbin/bluetoothd`, and finally `bluetoothd` through `PATH`.

Failure to determine the version is explicit: the report records `version=unknown` and emits a warning diagnostic. It does not silently invent or infer a package version.

## PAN capability observations

PANU is reported available only when all of these local prerequisites are present:

1. BlueZ owns `org.bluez` on the system D-Bus;
2. at least one Bluetooth adapter is powered;
3. Linux BNEP runtime support is present at `/sys/module/bnep`.

NAP additionally requires BlueZ managed objects to expose `org.bluez.NetworkServer1`. If BlueZ managed-object inspection fails, NAP is reported as unknown rather than guessed.

## Peer ceiling

P4-010 uses a conservative default of four active Bluetooth PAN links for initial topology policy. `DaemonConfig.topology.max_active_links` may lower that value. A configured value above the conservative runtime ceiling is diagnosed and capped for the effective capability report; it does not expand a hardware capability by configuration alone.

This is a policy ceiling, not a claim that every controller will sustain four links. P1/P16 hardware characterization remains authoritative for broader performance/capacity claims.

## Forwarding

The report reads `/proc/sys/net/ipv4/ip_forward` and validates that its value is `0` or `1`. It never writes the sysctl. P4-009 remains responsible for the ownership-aware forwarding lifecycle when routing is actually required.

NAT, masquerading, firewall policy, NetworkManager shared mode, and Internet gateway policy remain outside P4-010.

## Live probe

Build as the normal user:

```bash
cargo build -p blueroute-linux --example system_capability_probe --locked
```

Run without elevation first:

```bash
./target/debug/examples/system_capability_probe
```

The probe is read-only and should not require root on the supported Debian baseline. Its output is intended to be directly useful in diagnostics and later daemon/API status reporting.

## Acceptance expectations

Unit tests cover aggregate classification independently of physical hardware. Hardware acceptance should confirm that a known supported Debian machine reports the real BlueZ/NetworkManager versions, controller/driver, BNEP, PAN roles, forwarding state, peer ceiling, and kernel prerequisites, with diagnostics that explain the aggregate classification.

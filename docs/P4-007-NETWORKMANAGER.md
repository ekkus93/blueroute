# P4-007 — NetworkManager backend

P4-007 implements BlueRoute's initial Linux network backend through the NetworkManager system D-Bus API. Production code does **not** invoke or parse `nmcli`.

## Scope

`NetworkManagerBackend` implements the backend-neutral `NetworkStateBackend` and the P4-007 subset of `IpNetworkBackend`.

P4-007 owns:

- NetworkManager connection-profile enumeration;
- NetworkManager device enumeration;
- bounded connection/device change observation;
- BlueRoute-owned bridge profile creation and activation;
- BlueRoute-owned IPv4/IPv6 address configuration;
- owner-scoped address removal;
- owner-scoped profile deactivation/deletion;
- idempotent repeated ensure/remove operations.

P4-007 deliberately does **not** own route lifecycle or IPv4 forwarding. Those operations return typed `CapabilityUnavailable` errors until P4-008 and P4-009 implement them.

## D-Bus boundary

The backend talks directly to `org.freedesktop.NetworkManager` with `zbus`.

Relevant interfaces include:

- `org.freedesktop.NetworkManager`;
- `org.freedesktop.NetworkManager.Settings`;
- `org.freedesktop.NetworkManager.Settings.Connection`;
- `org.freedesktop.NetworkManager.Device`;
- `org.freedesktop.NetworkManager.Connection.Active`.

Connection creation uses `AddConnection2` with `to-disk` and `block-autoconnect`; updates use `Update2` with `to-disk`. Explicit activation is performed through `ActivateConnection`. Address changes on an already-active device are applied with `Device.Reapply`.

NetworkManager `address-data` is encoded as an array of variant dictionaries containing at least the string `address` and uint32 `prefix` fields. When BlueRoute writes `address-data`, it removes NetworkManager's deprecated `addresses` key first because NetworkManager ignores `address-data` when the legacy property is present in the same update.

## Ownership model

BlueRoute must never infer ownership from a connection name, UUID prefix, interface name, or NetworkManager object path.

Every BlueRoute-created profile carries namespaced metadata in NetworkManager's `user.data` dictionary:

- `org.blueroute.owner` — the canonical `NetworkId`;
- `org.blueroute.kind` — `bridge` or `interface`;
- `org.blueroute.schema` — ownership metadata schema version `1`.

A profile is treated as BlueRoute-owned only when the ownership metadata is complete and valid. Unknown ownership schemas, invalid owner identifiers, missing kinds, and unknown kinds fail closed.

Cleanup is scoped to both the requested `NetworkId` and interface. A profile owned by another BlueRoute network is not foreign in the general product sense, but it is **not owned by the current operation** and therefore cannot be adopted, overwritten, or removed.

Creating a bridge is also fail-closed when another NetworkManager profile already claims the requested interface. This includes a profile owned by a different BlueRoute `NetworkId`.

## State observation

`subscribe_network_state()` exposes D-Bus-neutral `NetworkStateEvent` values. The current implementation uses a bounded 250 ms reconciliation interval and a bounded event queue rather than leaking NetworkManager/zbus stream types through the crate boundary.

The initial subscription snapshot is emitted as `ConnectionAdded` / `DeviceAdded`, followed by deterministic add/change/remove differences.

## Hardware acceptance probe

The probe is:

```text
crates/blueroute-linux/examples/networkmanager_probe.rs
```

It uses only the Rust backend for NetworkManager operations. It does not invoke `nmcli`.

The probe exercises all P4-007 acceptance-critical behavior:

1. Connect to the live NetworkManager system D-Bus service and print its version.
2. Enumerate the baseline connection profiles and devices.
3. Remove only stale state from the probe's fixed owner, twice, to exercise idempotent cleanup.
4. Subscribe to NetworkManager state changes.
5. Create/activate `br-blue-nm` through `ensure_bridge()` and repeat the call, requiring the same profile handle.
6. Observe both the connection and virtual-device lifecycle through the Rust subscription.
7. Apply `10.254.89.1/30` through `ensure_address()` twice and require the backend to report it.
8. Verify baseline foreign NetworkManager profile identity/ownership is unchanged.
9. Attempt to claim the same bridge with a second `NetworkId`; this must fail with `InvalidState` **without creating a second profile**.
10. Attempt wrong-owner cleanup; it must be a no-op.
11. Hold the configured bridge long enough for an independent live-kernel inspection.
12. Remove the address twice, remove the owned bridge/profile twice, observe removal events, and verify foreign profiles remain.

The probe uses fixed test owners `47...47` and `48...48`; they are intentionally deterministic so a later run can safely clean up only its own state after an interrupted earlier run.

### Build and run on the hardware test node

Use the P4-007 branch revision being accepted:

```bash
cd ~/work/blueroute
git fetch origin
git switch P4-007_networkmanager_backend
git pull --ff-only origin P4-007_networkmanager_backend
git log -1 --oneline

cargo build -p blueroute-linux \
  --example networkmanager_probe \
  --locked

./target/debug/examples/networkmanager_probe br-blue-nm 90
```

If local NetworkManager/Polkit policy does not authorize the mutating D-Bus calls for the login session, build as the normal user and run **only the already-built probe binary** with elevation:

```bash
sudo ./target/debug/examples/networkmanager_probe br-blue-nm 90
```

Do not use `sudo cargo run`; that can leave root-owned build artifacts in the working tree.

### Live kernel inspection during the hold window

In a second terminal while the probe is holding the configured state:

```bash
ip link show br-blue-nm
ip -4 addr show br-blue-nm
```

Acceptance requires the bridge to exist and carry:

```text
10.254.89.1/30
```

The Rust probe itself must also have printed the connection/device observation, repeated ensure success, cross-owner rejection without a leaked profile, and wrong-owner cleanup no-op.

### Expected natural teardown

Do not press Ctrl-C during the acceptance run. Allow the hold window to expire so the Rust cleanup path is exercised.

A successful run ends with output equivalent to:

```text
address removed; repeated remove succeeded
observed NetworkManager connection removal for br-blue-nm
observed NetworkManager device removal for br-blue-nm
bridge/profile removed; repeated cleanup succeeded
foreign NetworkManager profiles preserved after cleanup
P4-007 NetworkManager probe PASS
```

After the probe exits, verify that its temporary bridge no longer exists:

```bash
ip link show br-blue-nm
```

`Device "br-blue-nm" does not exist` (or the equivalent `ip` failure) is the expected result.

## Acceptance evidence

Physical acceptance is recorded in `docs/P4-007-HARDWARE-EVIDENCE-2026-09-01.md`. On `debiancb1` (Debian 13, kernel `6.12.86+deb13-amd64`, BlueZ 5.82, NetworkManager 1.52.1), the Rust probe created and observed `br-blue-nm`, applied `10.254.89.1/30`, proved idempotent ensure/remove behavior, rejected a second BlueRoute owner without leaking a profile, treated wrong-owner cleanup as a no-op, and preserved all 15 baseline foreign NetworkManager profiles. The accepted run ended with `P4-007 NetworkManager probe PASS`.

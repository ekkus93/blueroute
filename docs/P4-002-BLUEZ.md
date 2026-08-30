# P4-002 BlueZ Adapter Discovery

BlueRoute discovers local Bluetooth controllers directly through BlueZ on the Linux system D-Bus. Production code does not invoke or parse `bluetoothctl`.

## D-Bus ownership

`blueroute_linux::BluezBackend` connects to the system bus with `zbus` and verifies that the well-known service name `org.bluez` currently has an owner. If BlueZ is unavailable, initialization fails with `ErrorKind::BluezUnavailable` rather than treating the machine as having an empty adapter list.

## Adapter enumeration

Initial state comes from `org.freedesktop.DBus.ObjectManager.GetManagedObjects` at `/` on `org.bluez`. Objects exposing `org.bluez.Adapter1` become `BluetoothAdapter` values.

The adapter handle is the full BlueZ object path (for example `/org/bluez/hci0`). BlueRoute does not infer capabilities or policy from the `hciN` suffix, computer model, or Bluetooth controller model.

The `org.bluez.Adapter1.Powered` property is mapped into `BluetoothAdapter::powered`. Returned adapters are sorted by handle so snapshots and tests are deterministic.

## Change observation

`BluetoothBackend::subscribe_adapter_events` exposes a D-Bus-neutral pull subscription. The BlueZ implementation listens for relevant BlueZ signals:

- `org.freedesktop.DBus.ObjectManager.InterfacesAdded`;
- `org.freedesktop.DBus.ObjectManager.InterfacesRemoved`;
- `org.freedesktop.DBus.Properties.PropertiesChanged` for `org.bluez.Adapter1`, when `Powered` changes or is invalidated.

After a relevant signal, BlueRoute obtains a fresh object-manager snapshot and diffs it against the previous snapshot. The generic events are:

- `BluetoothAdapterEvent::Added`;
- `BluetoothAdapterEvent::Removed`;
- `BluetoothAdapterEvent::PoweredChanged`.

Re-enumerating after a signal deliberately makes the object-manager snapshot authoritative and avoids duplicating partial BlueZ signal state in the domain boundary.

## Scope

P4-002 only covers BlueZ service availability, local adapter enumeration, adapter power state, and adapter-change observation. Nearby device discovery remains P4-003, pairing/trust remains P4-004, and PAN lifecycle remains P4-005/P4-006.

BlueZ daemon disappearance/restart recovery is intentionally not treated as ordinary adapter churn here. Reconciliation after a system-service restart belongs to the later reliability/reconciliation work, where daemon-wide backend state can be rebuilt consistently rather than partially repaired inside this discovery adapter.

GitHub Actions validates the D-Bus-independent parsing/diff logic and the Rust API integration. Physical-controller behavior remains part of the separate P1 hardware-characterization track and must not be inferred from CI alone.

# P4-003 BlueZ Device Discovery

BlueRoute performs nearby-device discovery directly through BlueZ on the Linux system D-Bus. Production code does not invoke or parse `bluetoothctl`.

## Discovery lifecycle

`BluezBackend::start_discovery` and `BluezBackend::stop_discovery` call `org.bluez.Adapter1.StartDiscovery` and `StopDiscovery` on a validated adapter object. BlueRoute verifies that the selected adapter still exists before each operation and rejects a start request with `ErrorKind::AdapterDisabled` when the adapter is powered off.

BlueZ owns discovery-session arbitration. A BlueRoute backend connection does not synthesize success for duplicate or unauthorized sessions; BlueZ method errors are mapped into typed `CoreError` categories such as `AdapterDisabled`, `InvalidState`, `MissingAdapter`, or `CapabilityUnavailable`.

## Device snapshots

`BluetoothBackend::discovered_peers` reads the authoritative BlueZ object-manager snapshot and selects immediate `org.bluez.Device1` children of the requested adapter.

Each device becomes a D-Bus-neutral `DiscoveredPeer` containing:

- the full BlueZ device object path as the opaque peer handle;
- display name from `Alias`, falling back to `Name` when needed;
- `Paired` state;
- `Trusted` state.

Results are sorted by peer handle for deterministic behavior. Missing required boolean properties and invalid property types fail explicitly as `ProtocolError` rather than being silently defaulted.

## Change events

`BluetoothBackend::subscribe_peer_events` exposes a D-Bus-neutral pull subscription with:

- `BluetoothPeerEvent::Added`;
- `BluetoothPeerEvent::Changed`;
- `BluetoothPeerEvent::Removed`.

The BlueZ implementation consumes `InterfacesAdded`, `InterfacesRemoved`, and relevant `PropertiesChanged` signals for `org.bluez.Device1`. A changed peer is refreshed from the object-manager snapshot before it is emitted so the event contains a complete domain value rather than a partial D-Bus property patch.

P4-003 deliberately reacts only to properties represented by `DiscoveredPeer` (`Alias`, `Name`, `Paired`, and `Trusted`). High-rate properties such as RSSI are not converted into peer-change events in this phase.

## Bounded state

The peer subscription does not accumulate its own historical peer map. It keeps only the bounded zbus signal queue already used by the BlueZ backend and refreshes current state on demand. `discovered_peers` returns a current snapshot and does not retain it after the call.

Stopping discovery releases BlueRoute's discovery session but does **not** call BlueZ `RemoveDevice`. Removing Device1 objects can also remove cached/bonding information, so using it as routine discovery-cache cleanup would be destructive.

## Hardware probe

`crates/blueroute-linux/examples/bluez_discovery_probe.rs` starts discovery on the first powered adapter for a fixed ten-second window, takes a Rust-backend peer snapshot, stops discovery, and prints the mapped peers. It is intended for the physical P4-003 acceptance check on Linux Bluetooth test systems.

Run it on a supported test node with another discoverable Linux/Bluetooth system nearby:

```bash
cargo run -p blueroute-linux --example bluez_discovery_probe --locked
```

CI validates the D-Bus-independent parsing, adapter scoping, property mapping, typed error mapping, and public trait integration. CI does not prove nearby-radio behavior; the TODO remains `[-]` until physical hardware evidence is recorded.

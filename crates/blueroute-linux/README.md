# blueroute-linux

Linux-specific system integration for BlueRoute.

The crate keeps Linux implementation details such as system D-Bus, BlueZ, persistence, and future network backends out of `blueroute-core`.

## BlueZ integration

`BluezBackend` uses the system D-Bus directly through `zbus`. It detects the `org.bluez` service, enumerates `org.bluez.Adapter1` objects, reads their `Powered` state, and observes adapter add/remove/power changes. It does not invoke `bluetoothctl`.

Device discovery uses `org.bluez.Adapter1.StartDiscovery` and `StopDiscovery`, maps `org.bluez.Device1` objects into D-Bus-neutral `DiscoveredPeer` values, and exposes peer add/change/remove events without maintaining an unbounded local peer cache. Pairing/trust and Bluetooth PAN lifecycle remain separate follow-on tasks.

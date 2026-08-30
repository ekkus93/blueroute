# blueroute-linux

Linux-specific system integration for BlueRoute.

The crate keeps Linux implementation details such as system D-Bus, BlueZ, persistence, and future network backends out of `blueroute-core`.

## BlueZ adapter discovery

`BluezBackend` uses the system D-Bus directly through `zbus` to detect the `org.bluez` service, enumerate `org.bluez.Adapter1` objects, read their `Powered` state, and observe adapter add/remove/power changes. It does not invoke `bluetoothctl`.

Device discovery, pairing/trust, and Bluetooth PAN lifecycle are intentionally separate follow-on tasks.

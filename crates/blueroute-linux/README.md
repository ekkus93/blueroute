# blueroute-linux

Linux-specific system integration for BlueRoute.

The crate keeps Linux implementation details such as system D-Bus, BlueZ, persistence, and network backends out of `blueroute-core`.

## BlueZ integration

`BluezBackend` uses the system D-Bus directly through `zbus`. It detects the `org.bluez` service, enumerates `org.bluez.Adapter1` objects, reads their `Powered` state, and observes adapter add/remove/power changes. It does not invoke `bluetoothctl`.

Device discovery uses `org.bluez.Adapter1.StartDiscovery` and `StopDiscovery`, maps `org.bluez.Device1` objects into D-Bus-neutral `DiscoveredPeer` values, and exposes peer add/change/remove events without maintaining an unbounded local peer cache. Pairing/trust uses a Rust-controlled BlueZ `Agent1` flow. PANU lifecycle uses `org.bluez.Network1.Connect("nap")`/`Disconnect()`, returns the BlueZ-created BNEP interface through `PanAttachment`, and exposes bounded link-loss events. NAP lifecycle uses `org.bluez.NetworkServer1.Register("nap", bridge)`/`Unregister("nap")`, tracks local registration ownership, and observes live Bluetooth bridge members for bounded client attach/detach events. Bridge creation and IP configuration remain separate network-backend responsibilities.

## NetworkManager integration

`NetworkManagerBackend` uses the NetworkManager system D-Bus API directly through `zbus`; production operations never invoke or parse `nmcli`. It enumerates connection profiles and devices, exposes bounded add/change/remove observation, creates and activates BlueRoute-owned bridge/interface profiles, and applies/removes BlueRoute-owned IP addresses.

Ownership is explicit in NetworkManager `user.data` metadata and scoped by BlueRoute `NetworkId`. The backend fails closed on malformed ownership metadata, conflicting foreign profiles, and profiles owned by another BlueRoute network. Cleanup removes only state carrying the requested BlueRoute owner metadata.

Route lifecycle and IPv4 forwarding remain intentionally unavailable until P4-008 and P4-009.

The live P4-007 acceptance procedure and `networkmanager_probe` are documented in `../../docs/P4-007-NETWORKMANAGER.md`.

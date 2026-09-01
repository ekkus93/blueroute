# blueroute-linux

Linux-specific system integration for BlueRoute.

The crate keeps Linux implementation details such as system D-Bus, BlueZ, persistence, and network backends out of `blueroute-core`.

## BlueZ integration

`BluezBackend` uses the system D-Bus directly through `zbus`. It detects the `org.bluez` service, enumerates `org.bluez.Adapter1` objects, reads their `Powered` state, and observes adapter add/remove/power changes. It does not invoke `bluetoothctl`.

Device discovery uses `org.bluez.Adapter1.StartDiscovery` and `StopDiscovery`, maps `org.bluez.Device1` objects into D-Bus-neutral `DiscoveredPeer` values, and exposes peer add/change/remove events without maintaining an unbounded local peer cache. Pairing/trust uses a Rust-controlled BlueZ `Agent1` flow. PANU lifecycle uses `org.bluez.Network1.Connect("nap")`/`Disconnect()`, returns the BlueZ-created BNEP interface through `PanAttachment`, and exposes bounded link-loss events. NAP lifecycle uses `org.bluez.NetworkServer1.Register("nap", bridge)`/`Unregister("nap")`, tracks local registration ownership, and observes live Bluetooth bridge members for bounded client attach/detach events. Bridge creation and IP configuration remain separate network-backend responsibilities.

## NetworkManager and kernel-network integration

`NetworkManagerBackend` uses the NetworkManager system D-Bus API directly through `zbus`; production operations never invoke or parse `nmcli`. It enumerates connection profiles and devices, exposes bounded add/change/remove observation, creates and activates BlueRoute-owned bridge/interface profiles, applies/removes BlueRoute-owned IP addresses, and manages BlueRoute-owned configured routes through NetworkManager `route-data`.

Ownership is explicit in NetworkManager `user.data` metadata and scoped by BlueRoute `NetworkId`. The backend fails closed on malformed ownership metadata, conflicting foreign profiles, profiles owned by another BlueRoute network, and route attributes that the current backend-neutral route model cannot preserve safely. Cleanup removes only state carrying the requested BlueRoute owner metadata.

Route ensure/remove is idempotent, changed next-hop/metric state is reconciled by destination, and durable configured routes are rediscovered after a fresh backend connection.

IPv4 forwarding is controlled separately through the Linux kernel `net.ipv4.ip_forward` setting. Because forwarding is node-global rather than connection-scoped, BlueRoute records the pre-existing value in a boot-local `/run/blueroute` lease before changing it. Repeated enable/release is idempotent, a fresh backend instance can recover the lease, and release without a BlueRoute lease never disables forwarding that may belong to another administrator or service. Forwarding does not configure NAT, firewall rules, masquerading, NetworkManager shared mode, or Internet gateway policy.

The live P4-007 NetworkManager acceptance procedure is documented in `../../docs/P4-007-NETWORKMANAGER.md`. P4-008 route design and hardware acceptance are documented in `../../docs/P4-008-ROUTES.md`. P4-009 forwarding ownership and hardware acceptance are documented in `../../docs/P4-009-FORWARDING.md`.

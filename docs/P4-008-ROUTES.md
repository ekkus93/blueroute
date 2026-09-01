# P4-008 — NetworkManager route adapter

P4-008 implements BlueRoute-owned route inspection and lifecycle through the existing NetworkManager system D-Bus backend. Production route operations do **not** invoke or parse `nmcli`, `ip route`, or other shell-command output.

## Scope

P4-008 implements the route subset of `IpNetworkBackend`:

- inspect BlueRoute-owned configured routes;
- add a route;
- reconcile a changed next hop or metric for an existing destination;
- remove an exact route;
- make repeated ensure/remove operations idempotent;
- rediscover durable route state through a fresh backend connection;
- preserve foreign NetworkManager profiles and routes outside BlueRoute-owned profiles.

IPv4 forwarding remains intentionally unavailable until P4-009.

## NetworkManager representation

Routes are stored in NetworkManager's modern `ipv4.route-data` / `ipv6.route-data` D-Bus properties. Each supported route entry maps to `LinuxRoute` using:

- `dest` — destination address;
- `prefix` — destination prefix length;
- `next-hop` — optional next hop;
- `metric` — explicit metric.

NetworkManager's deprecated `routes` property shadows modern `route-data`. When BlueRoute mutates route state in a profile it owns, the backend removes that legacy shadow before writing the modern representation. This is the route equivalent of the `addresses` / `address-data` compatibility issue discovered during P4-007 hardware acceptance.

## Ownership and safety boundary

Route ownership is profile-scoped. A route is BlueRoute-owned only when it is stored in a NetworkManager profile carrying valid BlueRoute `user.data` ownership metadata for a `NetworkId`.

P4-008 does **not** create a connection profile implicitly from `ensure_route()`. The requested interface must already have exactly one profile owned by the requested BlueRoute network. Missing ownership, duplicate owned profiles, another owner's profile, or a foreign profile causes the operation to fail closed rather than guess or adopt state.

`routes()` intentionally reports configured routes from BlueRoute-owned profiles; it does not claim ownership of arbitrary dynamic/kernel routes or routes in foreign NetworkManager profiles.

The current `LinuxRoute` model represents destination, optional next hop, interface, metric, and owner. If a BlueRoute-owned NetworkManager route contains additional route attributes that this model cannot represent safely, P4-008 fails closed instead of silently discarding those attributes during reconciliation.

Within one BlueRoute owner/interface, the destination prefix is the route identity for the current model. Calling `ensure_route()` with the same destination and a changed next hop or metric replaces stale variants instead of accumulating duplicate routes. `remove_route()` removes only the exact requested route.

## Address prerequisite

Before a route is added, the target IP family on the BlueRoute-owned profile must already use NetworkManager's `manual` method. In normal orchestration this means BlueRoute establishes the interface/profile and its owned address before applying routes. P4-008 does not silently change an unconfigured IP family solely because a route was requested.

## Backend reconnect/reconciliation

No route success is remembered only in process memory. `routes()` re-enumerates current NetworkManager profiles and parses their durable `route-data`. A newly constructed `NetworkManagerBackend` can therefore rediscover the configured route set and idempotently re-apply the desired route after a daemon/backend reconnect.

Full NetworkManager service-restart orchestration remains part of the later central reconciliation work, but P4-008 establishes the durable route-state boundary required for it.

## Hardware acceptance probe

The probe is:

```text
crates/blueroute-linux/examples/networkmanager_route_probe.rs
```

It uses only the Rust NetworkManager backend for route mutations. The probe:

1. removes only stale state belonging to its fixed probe owner;
2. records baseline foreign NetworkManager profiles;
3. creates `br-blue-rt` and applies `10.254.90.1/30`;
4. adds `10.254.91.0/24 via 10.254.90.2` with metric 177;
5. reconciles the same destination to metric 77 and repeats the ensure;
6. requires exactly one desired route and no stale metric-177 variant;
7. creates a fresh `NetworkManagerBackend`, rediscovers the route, and ensures it again;
8. rejects a different BlueRoute owner and verifies no leaked route state;
9. proves wrong-owner removal is a no-op;
10. holds the route for independent live-kernel inspection;
11. removes the route twice, then removes the address/profile twice;
12. verifies baseline foreign NetworkManager profiles remain unchanged.

The fixed probe owners are `49...49` and `4a...4a`; deterministic ownership allows a later run to clean only its own interrupted state.

### Build and run

On the Debian hardware test host, use the exact P4-008 branch revision being accepted:

```bash
cd ~/work/blueroute
git fetch origin
git switch P4-008_route_adapter
git pull --ff-only origin P4-008_route_adapter
git log -1 --oneline

cargo build -p blueroute-linux \
  --example networkmanager_route_probe \
  --locked

./target/debug/examples/networkmanager_route_probe br-blue-rt 90
```

As in P4-007, local NetworkManager/Polkit policy may allow read-only D-Bus calls but deny mutation to the login session. If that occurs, build as the normal user and run **only the already-built binary** with elevation:

```bash
sudo ./target/debug/examples/networkmanager_route_probe br-blue-rt 90
```

Do not run Cargo as root.

### Independent kernel inspection

Before starting the probe, capture the host's existing default route so it can be compared after cleanup:

```bash
ip -4 route show default
```

During the hold window, in a second terminal run:

```bash
ip -4 addr show br-blue-rt
ip -4 route show 10.254.91.0/24
```

Acceptance requires the bridge address `10.254.90.1/30` and a kernel route equivalent to:

```text
10.254.91.0/24 via 10.254.90.2 dev br-blue-rt proto static metric 77
```

A `linkdown` annotation is acceptable because this isolated P4-008 probe does not attach a carrier-producing PAN client to the bridge.

After the probe exits naturally:

```bash
ip link show br-blue-rt
ip -4 route show 10.254.91.0/24
ip -4 route show default
```

The temporary bridge must be absent, the probe route must be absent, and the pre-existing default route must remain unchanged.

## Acceptance status

Implementation/unit acceptance is complete when the locked workspace CI suite is green. P4-008 remains in progress until the physical NetworkManager/kernel route probe is run, foreign/default route preservation is verified, and the result is recorded in a dedicated hardware-evidence document.

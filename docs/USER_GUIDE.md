# BlueRoute user guide (development build)

**Status: not a finished consumer release.** BlueRoute has a Linux background service and a command-line client, both in the Debian package. A read-only terminal app and a desktop app exist but are run from a source checkout for now. Hosting and inspecting a local network work. **Joining is disabled in every packaged build** until it has passed a two-computer test on real hardware, so this is not yet a complete two-computer networking solution.

## What BlueRoute does

BlueRoute aims to let compatible Linux computers communicate using ordinary local TCP/IP over Bluetooth, without a Wi-Fi access point. One computer offers a network and another will eventually join it. Multi-hop routing and Internet sharing are not production-ready; Internet sharing is not enabled.

Compatibility depends on the **actual Bluetooth controller, driver, kernel and networking services**, not the laptop brand. A machine may be able to join as a client but not host. Dell Chromebook 3100 systems are development fixtures, not the product's hardware requirement.

## Before installing

The initial development and packaging baseline is **Debian 13** with BlueZ, NetworkManager, systemd and system D-Bus. Bluetooth must be enabled and the adapter must support the intended role. Other distributions and controllers are not automatically qualified. No general-availability installer exists yet.

### Controlled Debian development-package installation

1. Follow [development prerequisites](DEVELOPMENT.md) for Debian build packages and the pinned Rust toolchain.
2. From a checkout of the intended revision, run the locked checks:
   ```sh
   cargo fmt --all -- --check
   cargo check --workspace --locked
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```
3. Run `packaging/debian/build-package.sh`. The script validates its Debian package manifest and dependencies, **not** clean installation on your specific computer.
4. On a controlled Debian test machine with a matching amd64 architecture, install the generated `target/debian/blueroute_0.1.0_amd64.deb` with the Debian package manager. For example, from the checkout: `sudo apt install ./target/debian/blueroute_0.1.0_amd64.deb`.
5. Verify the installed service and client:
   ```sh
   systemctl status blueroute.service
   blueroute --version
   blueroute status
   ```

Installation should not create a network automatically. The daemon owns system networking and continues running independently of the CLI. **Do not delete `/var/lib/blueroute`** to fix an error: it contains persistent identity, membership and TLS keys. Clean-install and package-removal behavior still require installed-system acceptance.

## Check capabilities and local status

Run as your normal user:

```sh
blueroute status
blueroute capability show
blueroute diagnostics show --summary
```

`status` shows the local node, current network and health. `capability show` reports whether the local computer can host a network (NAP) or connect as a client (PANU), with evidence sources. **Unknown does not mean supported.** The reported connection-policy ceiling is not a measured Bluetooth client limit.

`diagnostics show --summary` is a short overview; `blueroute diagnostics show` gives more detail. On a computer hosting a network, "Authenticated control peers" counts the computers that hold a verified connection to it, against the configured limit (a setting, not a measured Bluetooth limit). Fields that say **unavailable** are not reported yet; routes and the network layout are among them. A healthy local daemon does not prove a peer connection or working traffic. `blueroute node list` reports the local node only.

### Terminal app (source checkout)

On a text terminal, run `cargo run -p blueroute-tui --locked` from the source checkout. It is a full-screen, **read-only** view of the local daemon: `1`–`4` (or Tab) switch between Overview, Networks, Devices and Diagnostics, arrow keys scroll, `r` refreshes, `h` shows help (Esc closes it) and `q` quits. It updates by itself when the daemon reports a change, and shows a banner while it has lost contact with the service.

It never changes anything. Changes need your password, which a full-screen terminal cannot ask for, so each screen shows the exact `blueroute` command to run in a normal shell instead. A missing or incompatible daemon is reported as unavailable, never as a made-up healthy state.

### Desktop app (source checkout)

The desktop app shows the same status in plain language. It can create and leave a network, ask a joined computer to reconnect to its host, rename this computer, and help diagnose problems. Every change may ask for your password. Joining is shown but disabled until it is available. Build instructions are in [DEVELOPMENT.md](DEVELOPMENT.md).

## Create a local network (host-only development flow)

On a host-capable computer:

```sh
blueroute network create "My local network"
blueroute network list
blueroute status
```

Network creation may prompt for PolicyKit authorization. It establishes hosted local state, **not** proof that a remote computer joined. The daemon manages BlueRoute-owned Bluetooth and IP resources; do not manually configure bridges or routes for the normal flow.

Discover nearby candidate networks with:

```sh
blueroute discover start
blueroute network list
blueroute discover stop
```

Discovery metadata is **not** authenticated identity or permission to join.

### Joining and peer trust: not yet available end-to-end

`blueroute network join NETWORK_ID` exists in the CLI. The authenticated join behind it is built, but every packaged build refuses it until it has passed a two-computer test on real hardware. There is no end-user join procedure today.

`blueroute peer trust NODE_ID` records legacy peer trust; it does **not** enroll a verified TLS key, authenticate a control session or grant membership. `blueroute peer local-key` and `blueroute peer verify-local-key` are read-only identity inspection/comparison aids; they do not enroll keys. Never accept an advertised fingerprint merely because it appeared in discovery.

When join becomes available, both computers confirm each other's keys out of band. On each computer, `blueroute peer local-key` shows its NodeId and key fingerprint. Read them aloud or compare them on screen, never from a nearby advertisement:

```sh
# on the joining computer: record the host it will trust
blueroute peer set-join-host NETWORK_ID HOST_NODE_ID HOST_FINGERPRINT
# on the host: trust the joining computer and pin its key
blueroute peer enroll JOINER_NODE_ID JOINER_FINGERPRINT
```

```sh
blueroute network link       # read-only: a joined client's link to its host
blueroute network reconnect  # try again after the link stopped retrying or needs attention
```

These two commands only matter for a joined client, which cannot exist yet because join is not available. Today `network link` reports that this computer is not a joined client.

### Leave a locally hosted network

```sh
blueroute network leave
blueroute status
```

Leave also works after a daemon restart in which the network could not be restarted: it removes only what carries this network's BlueRoute ownership. If cleanup fails, you stay in the network and can run leave again. A refused leave is not a successful teardown.

## Name this computer

```sh
blueroute node set-name "Kitchen laptop"
```

The name is only what people see. This computer's identity (its NodeId and keys) never changes, so networks and trusted devices are unaffected. The desktop app has the same option under Settings.

## Read device status

```sh
blueroute node list
blueroute diagnostics show
blueroute --json status
```

Node listings currently represent the **local node only**, not verified remote reachability. JSON output is for scripts; human output is for inspection. A visible network, adapter or local node is not evidence of authenticated remote membership.

## Common problems and safe recovery

| Symptom | First checks |
| --- | --- |
| Bluetooth off or missing | `blueroute capability show`, then `rfkill list` and system Bluetooth settings. |
| Cannot host | Inspect NAP capability; a PANU-only computer may eventually join but cannot necessarily host. |
| Daemon unavailable | `systemctl status blueroute.service` and `sudo journalctl -b -u blueroute.service`. |
| Status "Working, with problems" after Bluetooth or NetworkManager restarted | Wait about 15 seconds: a hosting computer checks itself and sets its network up again. If it stays degraded, check that both services are running. |
| Permission denied | Use the CLI's normal PolicyKit path; do not substitute `sudo blueroute`. |
| Visible network but join fails | Production join is disabled; discovery does not authenticate. |
| Address conflict | Inspect `blueroute diagnostics show` and `ip -4 route`; preserve unrelated routes. |
| Hosted network refuses leave | Preserve persistent state and collect logs; unknown ownership fails closed. |

See [troubleshooting](TROUBLESHOOTING.md) for driver/firmware problems, additional diagnosis and recovery limits. See the [administrator guide](ADMINISTRATOR.md) for complete CLI commands, JSON and service administration.

## What remains before the intended user experience

BlueRoute still needs independently verified key enrollment, production authenticated joining, joined-client leave/recovery, the full Ratatui TUI and desktop workflows and real multi-device Bluetooth acceptance. Physical tests must establish that traffic actually uses Bluetooth rather than Wi-Fi and document the exact tested Linux/controller combinations. This guide describes **currently available development behavior**, not a plug-and-play release.

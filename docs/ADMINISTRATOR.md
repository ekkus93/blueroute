# BlueRoute administrator guide

This guide covers the commands available in the current daemon and CLI. The CLI talks to the system daemon over the versioned system D-Bus API; it does not configure BlueZ or NetworkManager directly.

## Commands

Run `blueroute --help` for the installed version's command list and `blueroute --version` to see its version. Commands accept `--json` before or after the subcommand. JSON output is intended for scripts and preserves the daemon's typed response fields.

| Command | Purpose |
| --- | --- |
| `blueroute status` | Show API version, local node, current network, health, and local capabilities. |
| `blueroute capability show` | Show the daemon's local capability facts and their evidence source. |
| `blueroute diagnostics show` (`--summary` for a short form) | Show the current diagnostic snapshot: health, capabilities, platform versions, adapters, owned interface addresses on a host, the host's authenticated control peers against the policy ceiling, and recent API failures. |
| `blueroute network list` | List known networks. |
| `blueroute network create NAME` | Create and host a BlueRoute star. Privileged. |
| `blueroute network join NETWORK_ID` | Join a network. Refused as unsupported by every packaged build until two-machine hardware acceptance; see "Join on a test rig" below. |
| `blueroute network leave` | Leave the current network: tear down a hosted star, or a joined client's link, address and session. Privileged; retryable. |
| `blueroute network link` | Read-only: a joined client's link to its host (`connecting`, `waiting`, `established`, `needs_attention`, `gave_up`, or not a client). |
| `blueroute network reconnect` | Ask a joined client to try reaching its host again after it gave up or needed attention. Privileged. |
| `blueroute discover start` / `stop` | Start or stop BlueRoute network discovery. Privileged. |
| `blueroute node list` / `node show NODE_ID` | Inspect node records exposed by the daemon. Currently the local node only. |
| `blueroute node set-name NAME` | Rename this computer. A label only; the NodeId and keys never change. Privileged. |
| `blueroute peer local-key` / `peer verify-local-key NODE_ID FINGERPRINT` | Read-only: show, or compare against, this computer's NodeId, network and control-key fingerprint, for out-of-band confirmation. |
| `blueroute peer set-join-host NETWORK_ID NODE_ID FINGERPRINT` / `peer clear-join-host` | On a joining computer: record (or clear) the one host it will trust, confirmed out of band. Not membership. Privileged. |
| `blueroute peer enroll NODE_ID FINGERPRINT` | On a host: trust a joining computer and pin its control key, confirmed out of band. A different existing key is refused. Privileged. |
| `blueroute peer trust NODE_ID` | Record trust without a key. Grants no TLS access by itself. Privileged. |
| `blueroute peer forget NODE_ID` | Revoke trust and any key pin. Privileged. |

Examples:

```bash
blueroute --json status
blueroute capability show --json
blueroute diagnostics show
blueroute network list --json
blueroute node show 0123456789abcdef0123456789abcdef
```

The daemon enforces authorization (PolicyKit `org.blueroute.modify`) for every privileged command. Exit codes: `0` success, `1` operational failure, `2` usage, `3` daemon unavailable or incompatible, `4` permission denied, `5` unsupported; `--json` failures carry the same code. `network leave` tears down a hosted network. If the daemon restarted and could not restore the host, leave removes only the NetworkManager addresses and profiles that carry this network's BlueRoute ownership, then clears membership; nothing without that ownership is touched. A failed cleanup keeps membership, so leave can be retried. Joined-client leave and restart reconnection exist in the daemon but cannot be used until join is available.

## Capability and network diagnosis

Start with:

```bash
blueroute status
blueroute capability show
blueroute diagnostics show
```

Capabilities identify whether the local adapter is usable, whether PANU or NAP is available, whether the node can route, the selected network backend, and the conservative active-link ceiling. Each fact includes its source; unknown is distinct from false. These are local observations and do not establish that a remote peer is reachable or compatible.

For a more detailed read-only Linux probe, build and run:

```bash
cargo build -p blueroute-linux --example system_capability_probe --locked
./target/debug/examples/system_capability_probe
```

The probe reports BlueZ and NetworkManager availability, adapters, BNEP, PAN role prerequisites, forwarding state, kernel release, and diagnostics. It does not change networking state.

## Service logs

On systems installed with the packaged unit:

```bash
sudo systemctl status blueroute.service
sudo journalctl -b -u blueroute.service
sudo journalctl -f -u blueroute.service
```

The daemon runs as a system service, independent of a logged-in desktop session. Use `systemctl` to inspect or restart it. Do not delete files under `/var/lib/blueroute` as a troubleshooting step: they include stable identity, membership, and TLS key material.

Useful structured events (one JSON object per line, field `event`):

| Event | Meaning |
| --- | --- |
| `runtime_observation_updated` | Health or observed capabilities changed. |
| `host_runtime_repaired` | A hosted network was set up again after BlueZ or NetworkManager restarted, or its owned address was missing. |
| `host_runtime_check` (`outcome: failed`) | The hosted network is down and could not be repaired yet; logged once, not every 15 seconds. |
| `host_runtime_recovered` | The hosted network is up again after a failed check. |
| `startup_reconciliation`, `client_reconnect_start`, `device_name_load` | Startup steps that failed, with a coarse `failure_class`. |
| `experimental_join_enabled` | This is a hardware-test build with join open; never expected on a normal install. |

Request records never include payloads, peer-supplied labels, addresses or key material.

## Removing the package

`apt remove blueroute` stops the service, then runs `blueroute-daemon --remove-owned-networking` (root only, no other arguments). It removes the NetworkManager addresses and profiles, and a live PANU link it can prove is BlueRoute's, owned by the networks this computer knows, then checks that none are left. NetworkManager state of networks this computer does not know is left alone and counted as unresolved. Identity and membership in `/var/lib/blueroute` are kept; `apt purge` deletes them.

If cleanup cannot finish (NetworkManager unreachable, a profile that cannot be deleted, a 60-second timeout), the removal fails and the package stays installed with the service restarted. The journal line `"component":"removal_cleanup"` gives counts and a failure class. Fix the cause and run the removal again. Forcing the removal skips this guarantee. While a removal is pending the daemon refuses to start; reinstalling the package clears that. Upgrades do not tear networking down. This is tested with fakes and an isolated dpkg database; it has not yet been qualified on an installed system.

## Join on a test rig

Packaged and CI builds keep join closed. Two-machine acceptance uses a daemon built with the default-off `experimental-join` feature; its diagnostics version ends in `+experimental-join`. Instructions and checklists are in [HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md). Never install such a build outside the test rig.

## Current observability limits

The daemon does not report observed PANU links per peer, routes or a topology graph. On a host, authenticated control peers are counted, but a control session is not proof of a working data path. A local capability report is not proof of a working Bluetooth link. Use the physical acceptance procedures and evidence documents when validating radio and PAN behavior.

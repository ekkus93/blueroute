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

`apt remove blueroute` stops the service, then runs `blueroute-daemon --remove-owned-networking` (root only, no other arguments). It removes the NetworkManager addresses and profiles owned by the networks this computer knows, then checks that none are left. On a computer that joined a network as a client it must also see its Bluetooth link: it disconnects the link only when it can prove it is BlueRoute's, and removes that network's profile and address only once the link is gone. If Bluetooth is not running, that network is left alone (`unresolved_client_links`) and the removal fails; start `bluetooth.service` and remove again. NetworkManager state of networks this computer does not know is left alone and counted as unresolved. A network whose role cannot be established is also left alone (`unresolved_roles`) and fails the removal; see [below](#removal-with-an-unresolved-role). Identity and membership in `/var/lib/blueroute` are kept; `apt purge` deletes them.

If cleanup cannot finish (NetworkManager unreachable, a profile that cannot be deleted, a 60-second timeout), the removal fails and the package stays installed with the service restarted. The cleanup prints one `"component":"removal_cleanup"` line with counts and a failure class to the apt/dpkg output (also kept in `/var/log/apt/term.log`), not to the journal. Fix the cause and run the removal again. Forcing the removal skips this guarantee. While a removal is pending the daemon refuses to start; reinstalling the package clears that. Upgrades do not tear networking down. This is tested with fakes and an isolated dpkg database; it has not yet been qualified on an installed system.

### Removal with an unresolved role

The cleanup line reports `unresolved_roles` when the removal fails because this computer knows a network
whose role it cannot establish. This happens when the membership is `member` with no stored role, or
when a client record names a network with no membership. The current daemon never writes either,
so they come from damaged or hand-edited state in `/var/lib/blueroute`. Cleanup then touches nothing
for that network: it cannot tell whether a Bluetooth PAN link of a joined client is still up, and
that network's NetworkManager profile and address are the proof that such a link is BlueRoute's.
Starting Bluetooth does not change this. The removal keeps failing until the role question is
settled; do not guess it.

Recovery is manual and outside the cleanup's success guarantee. With the service stopped
(`systemctl stop blueroute.service`):

1. Find the network's NetworkId (`network` lines of `memberships-v1`, or the client record in
   `client-record/client-record-v1`).
2. Confirm that no Bluetooth PAN link of that network is up. Check for a `bnep` interface carrying
   a NetworkManager profile whose `user.data` has `org.blueroute.owner` set to that NetworkId,
   and check the Bluetooth device it connects to. If a link is up and is BlueRoute's, disconnect it.
3. Delete the NetworkManager profiles (and with them their addresses) whose `org.blueroute.owner`
   is that NetworkId. Never delete a profile without that ownership.
4. Remove what made the role unresolved. For a client record with no membership, delete
   `client-record/client-record-v1`. For a `member` network with no role, delete that network's
   `network` line and every `peer` line for the same NetworkId from `memberships-v1`.
5. Run the removal again. Any owned state you did not delete is still listed, as another
   installation's state, under `unresolved_foreign_resources`.

## Join on a test rig

Packaged and CI builds keep join closed. Two-machine acceptance uses a daemon built with the default-off `experimental-join` feature; its diagnostics version ends in `+experimental-join`. Instructions and checklists are in [HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md). Never install such a build outside the test rig.

## Current observability limits

The daemon does not report observed PANU links per peer, routes or a topology graph. On a host, authenticated control peers are counted, but a control session is not proof of a working data path. A local capability report is not proof of a working Bluetooth link. Use the physical acceptance procedures and evidence documents when validating radio and PAN behavior.

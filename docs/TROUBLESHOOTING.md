# BlueRoute troubleshooting

BlueRoute depends on the system BlueZ service, a usable Bluetooth adapter, BNEP support, NetworkManager, system D-Bus, and systemd on the initial Debian baseline. Start with the daemon's read-only reports:

```bash
blueroute status
blueroute capability show
blueroute diagnostics show
sudo systemctl status blueroute.service
sudo journalctl -b -u blueroute.service
```

For adapter, driver, firmware, and backend details, run the read-only [system capability probe](P4-010-CAPABILITIES.md#live-probe). Do not infer support from a computer model name; runtime controller capabilities vary.

## Common failures

### Bluetooth is disabled or no adapter is present

Check `blueroute capability show` for `adapter_usable`, then inspect `bluetoothctl show` and `rfkill list`. Enable Bluetooth through the system's normal administrator controls, then restart discovery. If no controller appears, inspect the system journal and the capability probe for kernel driver and sysfs details.

### The adapter cannot host a network

`nap` reports local NAP server support; `panu` reports local client prerequisites. A node may be able to join as a PANU client while being unable to host as NAP. Do not interpret a missing or unknown NAP capability as a general Bluetooth failure. Remote peer compatibility is only established by an actual connection attempt.

The daemon reprobes capabilities every 15 seconds, and creating a network checks the latest result, so after you fix the adapter (power it on, reload the driver) creation works without restarting BlueRoute once status shows hosting as available. A probe that fails reports hosting as unknown, which refuses creation until a later probe succeeds.

### BlueZ or NetworkManager is unavailable

Check `systemctl status bluetooth.service NetworkManager.service` and `journalctl -b -u bluetooth.service -u NetworkManager.service`. BlueRoute reports missing services as degraded or unsupported and does not silently substitute shell commands or another backend.

On a computer hosting a network, BlueRoute checks itself every 15 seconds. After BlueZ or NetworkManager restarts, it sets the network up again by itself (the log shows `host_runtime_repaired`) and health returns to normal; no BlueRoute restart is needed. If health stays degraded and the log shows `host_runtime_check` with `outcome: failed`, the services are still down or the repair failed; its `failure_class` says which.

### Pairing fails

Confirm both devices are discoverable as required by their local policy, then inspect the BlueZ and BlueRoute daemon logs. Pairing/trust approval is separate from cryptographic control-channel key enrollment and network membership. Repeatedly approving a peer does not resolve an unsupported adapter role or an authenticated-session failure.

### A host refuses a joining computer

The host admits only computers whose key was enrolled with `blueroute peer enroll`, up to its configured client limit (`connection_policy_ceiling`, shown in diagnostics with its source). A computer over the limit gets a session refusal while its Bluetooth link may stay up. Check that the enrolled fingerprint matches the joining computer's own `blueroute peer local-key`, and how many authenticated peers diagnostics already shows.

### Permission denied

Mutating daemon operations require system authorization. Retry through the supported CLI so the PolicyKit authorization path can run. Read-only commands should not require elevated privileges. Do not run the CLI with `sudo` as a workaround; it can hide the authorization problem and does not replace daemon policy.

### The daemon is unavailable or reports unsupported capability

Use `systemctl status blueroute.service`, inspect `journalctl -b -u blueroute.service`, and compare `blueroute --version` with the installed daemon package. CLI exit status `3` indicates unavailable/incompatible daemon; `5` indicates an unsupported capability; `4` indicates authorization denial.

### Peer is unreachable or the link is stale

Check that discovery sees the expected BlueRoute network and that both nodes have powered adapters. Discovery metadata is not authentication: a listed network is not a member and has no control session by being listed.

On a host, `blueroute diagnostics show` lists the computers holding an authenticated control session. On a joined client, `blueroute network link` shows the link to the host:

- `connecting` / `waiting`: BlueRoute is retrying with growing delays; nothing to do yet.
- `needs_attention`: the reason says what to fix (for example a missing host key); fix it, then run `blueroute network reconnect`.
- `gave_up`: the host was unreachable for every attempt; membership is kept. When the host is back, run `blueroute network reconnect`.

Joining is not available in packaged builds yet, so these states only appear on a test rig. Capture daemon, BlueZ and NetworkManager logs plus capability-probe output for investigation.

### Address conflict

BlueRoute refuses an IPv4 segment that overlaps an observed active local prefix. The CLI then says so specifically (JSON `"kind": "address_conflict"`, exit status 1) and the desktop app explains it in plain words; both come from a typed daemon error, not from message text. Common causes are a VPN, a container or virtual-machine network, or another connection using the same private range. Inspect `ip -4 address` and `ip -4 route` before changing the configured pool. Preserve unrelated host routes and do not manually delete a route unless its ownership is established. A failed join remains fail-closed and should not be reported as membership.

### Routed peer is unavailable

Multi-hop route distribution and production routed topology are not enabled yet. Current routing capability is a local capability fact, not evidence that a peer route exists. Do not add a default route or enable Internet sharing to work around a missing BlueRoute route.

### Driver or firmware quirks

Record the distribution, kernel, BlueZ and NetworkManager versions, controller identifiers, driver and firmware, and the exact reproduction procedure. Use the [capability matrix](platforms/CAPABILITY_MATRIX.md) and hardware acceptance backlog to record results. Keep hardware-specific workarounds behind runtime capability/backend boundaries.

## Recovery limits

The daemon intentionally fails closed when persistent identity, membership, peer-pin, or TLS identity state is corrupt or insecure. Preserve the state directory and logs for diagnosis. Do not remove `/var/lib/blueroute` to make startup succeed; doing so can discard identity and trust state. Contact the project maintainer with the sanitized logs and system capability report if repair is needed.

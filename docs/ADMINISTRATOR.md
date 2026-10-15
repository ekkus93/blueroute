# BlueRoute administrator guide

This guide covers the commands available in the current daemon and CLI. The CLI talks to the system daemon over the versioned system D-Bus API; it does not configure BlueZ or NetworkManager directly.

## Commands

Run `blueroute --help` for the installed version's command list and `blueroute --version` to see its version. Commands accept `--json` before or after the subcommand. JSON output is intended for scripts and preserves the daemon's typed response fields.

| Command | Purpose |
| --- | --- |
| `blueroute status` | Show API version, local node, current network, health, and local capabilities. |
| `blueroute capability show` | Show the daemon's local capability facts and their evidence source. |
| `blueroute diagnostics show` | Show the current diagnostic snapshot. Node counts are local-only until authenticated peer runtime is integrated. |
| `blueroute network list` | List known networks. |
| `blueroute network create NAME` | Request creation of a BlueRoute star. This is a privileged operation. |
| `blueroute network join NETWORK_ID` | Request a network join. The current daemon intentionally rejects this as unsupported until authenticated peer transport and production join runtime are integrated. |
| `blueroute network leave` | Stop a locally hosted network whose runtime is still owned by this daemon. This is privileged and idempotent after leave succeeds. |
| `blueroute discover start` / `stop` | Start or stop BlueRoute network discovery. |
| `blueroute node list` / `node show NODE_ID` | Inspect node records currently exposed by the daemon. Current runtime reports the local node only. |
| `blueroute peer trust NODE_ID` | Record explicit legacy peer trust. This is not TLS-key enrollment or network membership. |
| `blueroute peer forget NODE_ID` | Revoke peer trust and any configured key pin. |

Examples:

```bash
blueroute --json status
blueroute capability show --json
blueroute diagnostics show
blueroute network list --json
blueroute node show 0123456789abcdef0123456789abcdef
```

The daemon enforces authorization for mutations. An authorization failure is reported separately from an unavailable daemon or unsupported capability. Production `network join` remains fail-closed until authenticated peer transport is integrated. `network leave` currently supports the local host runtime only; it refuses to clear persisted membership if this daemon cannot confirm ownership of an active host runtime, including after a daemon restart. Client-side leave and restart reconciliation remain unimplemented.

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

## Current observability limits

The daemon does not yet expose a live authenticated remote-peer inventory, routed topology, interface/address/route snapshot, or active TLS session state. A local capability report is not proof of a working Bluetooth data path. Use the physical acceptance procedures and evidence documents when validating radio and PAN behavior.

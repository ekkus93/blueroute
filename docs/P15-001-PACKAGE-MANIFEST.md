# P15-001 — Debian package contents manifest

This manifest defines the intended `blueroute` binary package layout for Debian 13. It is a packaging contract, not evidence that a `.deb` has been built or installed. P15-002..005 qualify build and lifecycle behavior separately.

## Runtime files

| Source | Installed path | Owner / mode | Purpose |
| --- | --- | --- | --- |
| `target/release/blueroute-daemon` | `/usr/libexec/blueroute/blueroute-daemon` | `root:root`, `0755` | Privileged system D-Bus daemon. |
| `target/release/blueroute` | `/usr/bin/blueroute` | `root:root`, `0755` | Unprivileged CLI client. |
| `target/release/blueroute-tui` | `/usr/bin/blueroute-tui` | `root:root`, `0755` | TUI client; install once its P11 screens are functional. |
| `packaging/systemd/blueroute.service` | `/usr/lib/systemd/system/blueroute.service` | `root:root`, `0644` | Root service, journald output, systemd-managed state/runtime directories. |
| `packaging/dbus/org.blueroute.Service1.conf` | `/usr/share/dbus-1/system.d/org.blueroute.Service1.conf` | `root:root`, `0644` | Root-only service-name ownership and local client method-call policy. |
| `packaging/polkit/org.blueroute.policy` | `/usr/share/polkit-1/actions/org.blueroute.policy` | `root:root`, `0644` | Explicit administrator authorization for mutations. |

P12's desktop application is currently an empty Rust shell. It must not be presented as a usable application or installed as a desktop launcher until P12-001..P12-012 deliver its bridge, workflows, metadata and artwork. The reserved future paths are `/usr/bin/blueroute-desktop`, `/usr/share/applications/org.blueroute.desktop`, and `/usr/share/icons/hicolor/scalable/apps/org.blueroute.svg`.

## Runtime dependencies

- `systemd` and system D-Bus for daemon activation and service ownership;
- `polkitd` plus an authentication agent for authorized interactive mutations;
- BlueZ and NetworkManager for the initial Bluetooth PAN and Linux network backends.

The CLI/TUI use the shared daemon client; they do not require direct BlueZ or NetworkManager privileges. The initial backend dependency must remain replaceable without moving Linux service policy into `blueroute-core`.

## Persistent and generated state

The package does not ship machine-specific state. systemd creates `/var/lib/blueroute` as a root-owned `0700` state directory and `/run/blueroute` as a root-owned `0700` runtime directory. The package must not create a network, enable forwarding, install NAT/firewall rules, or enroll peers during installation. First-run startup creates only the daemon's stable local identities and reads capability state.

Package removal stops the service and removes package-owned system policy and binaries. It retains `/var/lib/blueroute` by default so removal cannot silently rotate identity or erase trust; explicit purge policy and any generated runtime cleanup are specified and tested under P15-005.

## Reproducibility boundary

P15-002 should generate this file list from source paths and a clean locked release build, then compare the resulting package contents to this manifest. TUI and desktop deliverables stay gated on their respective implementation tasks; they must not be replaced by empty stub binaries in a release package.

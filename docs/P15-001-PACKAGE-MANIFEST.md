# P15-001 — Debian package contents manifest

This manifest defines the `blueroute` and `blueroute-desktop` binary package layouts for Debian 13. It is a packaging contract, not evidence that a `.deb` has been built or installed. P15-002..005 qualify build and lifecycle behavior separately.

## Runtime files

| Source | Installed path | Owner / mode | Purpose |
| --- | --- | --- | --- |
| `target/release/blueroute-daemon` | `/usr/libexec/blueroute/blueroute-daemon` | `root:root`, `0755` | Privileged system D-Bus daemon. |
| `target/release/blueroute` | `/usr/bin/blueroute` | `root:root`, `0755` | Unprivileged CLI client. |
| `target/release/blueroute-tui` | `/usr/bin/blueroute-tui` | `root:root`, `0755` | Read-only full-screen terminal client (shipped since 2026-11-12). |
| `packaging/systemd/blueroute.service` | `/usr/lib/systemd/system/blueroute.service` | `root:root`, `0644` | Root service, journald output, systemd-managed state/runtime directories. |
| `packaging/dbus/org.blueroute.Service1.conf` | `/usr/share/dbus-1/system.d/org.blueroute.Service1.conf` | `root:root`, `0644` | Root-only service-name ownership and local client method-call policy. |
| `packaging/polkit/org.blueroute.policy` | `/usr/share/polkit-1/actions/org.blueroute.policy` | `root:root`, `0644` | Explicit administrator authorization for mutations. |

## `blueroute-desktop` package (2026-11-12)

The desktop app ships as a **separate** package so daemon-only machines do not pull in WebKitGTK and GTK (maintainer decision, 2026-11-12).

| Source | Installed path | Owner / mode | Purpose |
| --- | --- | --- | --- |
| `target/release/blueroute-desktop-app` (built with `--features custom-protocol`) | `/usr/bin/blueroute-desktop` | `root:root`, `0755` | Desktop app; embeds its built frontend and talks to the daemon only through its fixed Tauri commands. |
| `packaging/desktop/org.blueroute.desktop` | `/usr/share/applications/org.blueroute.desktop` | `root:root`, `0644` | Application launcher (validated with `desktop-file-validate`). |
| `apps/desktop/src-tauri/icons/128x128.png` | `/usr/share/icons/hicolor/128x128/apps/org.blueroute.png` | `root:root`, `0644` | Launcher icon. |

It depends on `blueroute` at exactly the same version, plus the shared libraries `dpkg-shlibdeps` computes from the binary (WebKitGTK 4.1, GTK 3, libsoup 3 and their base libraries). The build script checks the binary embeds the frontend it just built, so it can never ship pointing at the development server. The package must be built on the target distribution so the computed library package names match it.

## Runtime dependencies

- `systemd` and system D-Bus for daemon activation and service ownership;
- `polkitd` plus an authentication agent for authorized interactive mutations;
- BlueZ and NetworkManager for the initial Bluetooth PAN and Linux network backends.

The CLI/TUI use the shared daemon client; they do not require direct BlueZ or NetworkManager privileges. The initial backend dependency must remain replaceable without moving Linux service policy into `blueroute-core`.

## Persistent and generated state

The package does not ship machine-specific state. systemd creates `/var/lib/blueroute` as a root-owned `0700` state directory and `/run/blueroute` as a root-owned `0700` runtime directory. The package must not create a network, enable forwarding, install NAT/firewall rules, or enroll peers during installation. First-run startup creates only the daemon's stable local identities and reads capability state.

Package removal stops the service and removes package-owned system policy and binaries. It retains `/var/lib/blueroute` by default so removal cannot silently rotate identity or erase trust; explicit purge policy and any generated runtime cleanup are specified and tested under P15-005.

## Reproducibility boundary

P15-002 should generate this file list from source paths and a clean locked release build, then compare the resulting package contents to this manifest. Both packages' file lists are generated and compared by `packaging/debian/build-package.sh`, which also checks each package's root directory mode.

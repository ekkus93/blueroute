# Distribution packaging boundary

BlueRoute's domain and protocol crates define network identity, membership, topology, commands and policy-independent behavior. Linux service integration lives in `blueroute-linux`; the daemon and clients consume those interfaces. Distribution packaging must not move service manager, package database, filesystem layout or package lifecycle decisions into `blueroute-core` or `blueroute-protocol`.

## Debian-specific integration

The current Debian package is built from `packaging/debian/` and installs files under `/usr/bin`, `/usr/libexec`, `/usr/lib/systemd/system`, `/usr/share/dbus-1/system.d`, and `/usr/share/polkit-1/actions`. Its runtime dependencies are systemd, system D-Bus, BlueZ, NetworkManager and PolicyKit. Debian package metadata, service enablement, package hooks and Debian-specific paths belong in this packaging layer.

The daemon's runtime state contract is `/var/lib/blueroute` for durable state and `/run/blueroute` for volatile state, with root-only access. The service manager creates these directories. The daemon may rely on its configured state paths and Linux adapter interfaces, but must not inspect dpkg state or invoke Debian package tools.

## Requirements for another distribution

A new package should provide equivalent capabilities for:

- installing the daemon and selected clients at conventional executable paths;
- starting the daemon as a privileged system service after system D-Bus, BlueZ and the selected network backend are available;
- restricting ownership of the BlueRoute system-bus name to the daemon and installing PolicyKit authorization metadata or an equivalent supported authorization mechanism;
- creating private persistent and runtime directories with the required ownership and permissions;
- declaring equivalent runtime dependencies and removing package-owned service/policy files on uninstall;
- preserving `/var/lib/blueroute` data by default on package removal, unless the distribution's explicit purge operation requests deletion.

Distribution adapters and package hooks must preserve the same no-network-on-install behavior and ownership-aware cleanup contract. A distribution that lacks an equivalent authorization or service boundary is not supported by packaging alone; it needs an explicit architecture review before being treated as supported.

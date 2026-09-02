# P5-006 — systemd Service

P5-006 packages `blueroute-daemon` as a system-level systemd service on the initial Debian 13 baseline. The service is intentionally independent of graphical/user sessions: BlueRoute owns a system D-Bus API and its networking state must remain alive when a desktop user logs out.

## Installed layout

The packaged layout is:

- daemon executable: `/usr/libexec/blueroute/blueroute-daemon`
- unit: `/usr/lib/systemd/system/blueroute.service`
- D-Bus ownership policy: `/usr/share/dbus-1/system.d/org.blueroute.Service1.conf`
- durable state: `/var/lib/blueroute`
- boot-local runtime state: `/run/blueroute`

`StateDirectory=blueroute` and `RuntimeDirectory=blueroute` make systemd create the state/runtime directories before daemon startup. Both use mode `0700`, and the service uses `UMask=0077` so newly created private state is not accidentally world-readable.

## Service readiness

The unit uses:

```ini
Type=dbus
BusName=org.blueroute.Service1
```

This is stronger than declaring the process ready immediately after `execve`: systemd considers startup complete only when the daemon actually owns the versioned BlueRoute system-bus name.

The service runs as root. That is deliberate for the privileged networking daemon: the Linux adapters ultimately need controlled access to BlueZ, NetworkManager, route/forwarding state, and `/run/blueroute`. This does **not** authorize front ends to perform arbitrary privileged operations. P5-007 owns D-Bus/Polkit caller authorization, and the UI/CLI are not run as root.

`NoNewPrivileges=yes` prevents the daemon or a child process from gaining additional privileges through a later exec transition. `PrivateTmp=yes` isolates its temporary files without isolating the host network namespace.

## System D-Bus name ownership

The system bus denies arbitrary well-known-name ownership by default. P5-006 therefore ships `packaging/dbus/org.blueroute.Service1.conf` with the narrow policy required for the root-run daemon to own exactly `org.blueroute.Service1`:

```xml
<policy user="root">
  <allow own="org.blueroute.Service1"/>
</policy>
```

This policy is intentionally limited to **service-name ownership**. It contains no wildcard ownership, no broad send/receive grant, and no method-level authorization rule. P5-007 remains responsible for distinguishing read-only from mutating operations and enforcing caller authorization/Polkit policy.

Live P5-006 testing exposed this packaging requirement directly: without the ownership policy, the daemon failed closed with `org.freedesktop.DBus.Error.AccessDenied` rather than silently falling back to a session bus or a different service name. That failure mode is correct; the missing packaging policy was the defect.

## Startup ordering

The unit is enabled under `multi-user.target`, not a graphical-session target:

```ini
WantedBy=multi-user.target
```

It orders itself after the system bus, BlueZ, and NetworkManager and weakly wants the two networking services:

```ini
Wants=bluetooth.service NetworkManager.service
After=dbus.service bluetooth.service NetworkManager.service
```

`Wants`, rather than `Requires`, is intentional. The daemon's capability model can report missing/degraded platform services; a missing optional capability should not be converted into an opaque systemd dependency failure.

## Restart policy

Unexpected daemon failure is automatically restarted:

```ini
Restart=on-failure
RestartSec=2s
```

Normal administrative stops are not restarted. `StartLimitIntervalSec=30s` with `StartLimitBurst=5` bounds a persistent crash loop instead of retrying indefinitely at high frequency.

## Logging

stdout and stderr are explicitly routed to journald under the stable identifier `blueroute`:

```ini
StandardOutput=journal
StandardError=journal
SyslogIdentifier=blueroute
```

Useful inspection commands are:

```bash
sudo systemctl status blueroute.service
sudo journalctl -u blueroute.service
sudo journalctl -b -u blueroute.service
```

The daemon must report startup failures rather than silently falling back; those errors therefore remain visible in the service journal.

## Development installation on Debian

For P5-006 hardware acceptance from a repository checkout:

```bash
cargo build --release -p blueroute-daemon --locked

sudo install -Dm0755 \
  target/release/blueroute-daemon \
  /usr/libexec/blueroute/blueroute-daemon
sudo install -Dm0644 \
  packaging/systemd/blueroute.service \
  /usr/lib/systemd/system/blueroute.service
sudo install -Dm0644 \
  packaging/dbus/org.blueroute.Service1.conf \
  /usr/share/dbus-1/system.d/org.blueroute.Service1.conf

sudo systemctl daemon-reload
sudo systemctl reload dbus.service
# A previous failed acceptance run may have exhausted StartLimitBurst.
sudo systemctl reset-failed blueroute.service
sudo systemctl enable --now blueroute.service
```

The unit and D-Bus policy files must be installed unchanged for acceptance; substituting a different `ExecStart`, target, restart policy, service name, or broader bus policy would not validate the shipped artifacts.

## Acceptance procedure

Static/CI acceptance verifies the unit syntax, critical systemd policy settings, and that the D-Bus policy grants only root ownership of `org.blueroute.Service1`. Live Debian acceptance must additionally prove all of the following on the supported baseline:

1. `blueroute.service` becomes `active` and owns `org.blueroute.Service1`.
2. `/var/lib/blueroute` and `/run/blueroute` are systemd-managed with restrictive permissions.
3. a forced unexpected daemon failure is restarted according to `Restart=on-failure`.
4. after a reboot, the enabled service starts automatically under `multi-user.target` and has current-boot journal entries.
5. after GUI logout and login, the system service remains active independently of the user session.

P5-006 is not complete until the boot and GUI-logout behavior is recorded from real Debian hardware.

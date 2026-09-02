# P5-006 Hardware Acceptance — 2026-09-01

## Scope

This record closes the physical Debian acceptance for **P5-006 — Add systemd service**. The test validates the shipped system-level `blueroute.service`, its system D-Bus ownership policy, restart behavior, boot activation, journald integration, systemd-managed directories, and independence from the graphical login session.

The test does not claim that P5-007 caller authorization/Polkit policy is complete. P5-006 only grants the root-run daemon permission to own the well-known D-Bus name; method-level caller authorization remains P5-007.

## Test system

- Host: `debiancb1`
- Distribution: Debian GNU/Linux 13 (trixie)
- Kernel: `6.12.86+deb13-amd64`
- BlueZ: `5.82`
- NetworkManager: `1.52.1`
- systemd: `257 (257.9-1~deb13u1)`
- P5-006 branch: `P5-006_systemd_service`
- Final acceptance candidate: `454265f7dc06bd500d0ee621dfc41d90b3d38ad2`
- Service executable: `/usr/libexec/blueroute/blueroute-daemon`
- Unit: `/usr/lib/systemd/system/blueroute.service`
- D-Bus ownership policy: `/usr/share/dbus-1/system.d/org.blueroute.Service1.conf`

## Initial fail-closed discovery

The first live start correctly failed instead of silently falling back. Journald reported:

```text
BlueRoute daemon failed: org.freedesktop.DBus.Error.AccessDenied: Connection ... is not allowed to own the service "org.blueroute.Service1" due to security policies in the configuration file
```

systemd applied the configured restart policy until the bounded start limit was reached (`NRestarts=5`). This exposed a packaging omission: the system bus had no policy permitting the root-run daemon to own `org.blueroute.Service1`.

The fix added `packaging/dbus/org.blueroute.Service1.conf`, narrowly granting only `root` permission to own that single well-known name. No general client send permission or method-level authorization was added.

## Service startup and D-Bus readiness

After installing the D-Bus ownership policy and restarting the service:

```text
systemctl is-enabled blueroute.service
=> enabled

systemctl is-active blueroute.service
=> active
```

`busctl --system status org.blueroute.Service1` resolved a live owner in `system.slice` with no user unit or user session association. Because the unit uses `Type=dbus` with `BusName=org.blueroute.Service1`, the `active` state proves the daemon acquired the required system-bus name rather than merely surviving `execve`.

## Managed state and runtime directories

The live directories were systemd-managed with the expected restrictive permissions:

```text
700 root:root /var/lib/blueroute
700 root:root /run/blueroute
```

The service reported:

```text
MainPID=11248
Result=success
NRestarts=0
```

The current-boot journal contained the successful startup transition:

```text
Starting blueroute.service - BlueRoute Bluetooth PAN networking daemon...
Started blueroute.service - BlueRoute Bluetooth PAN networking daemon.
```

This also proves daemon stderr/stdout failures and systemd lifecycle messages remain visible through journald.

## Unexpected-failure restart

The running daemon PID was captured and deliberately killed with `SIGKILL`:

```text
PID before crash: 11248
PID after restart: 11295
active
Result=success
NRestarts=1
```

The journal recorded:

```text
blueroute.service: Main process exited, code=killed, status=9/KILL
blueroute.service: Failed with result 'signal'.
blueroute.service: Scheduled restart job, restart counter is at 1.
Starting blueroute.service - BlueRoute Bluetooth PAN networking daemon...
Started blueroute.service - BlueRoute Bluetooth PAN networking daemon.
```

This proves the shipped `Restart=on-failure` policy restarts an unexpectedly terminated daemon and reacquires readiness.

## Boot activation

`debiancb1` was rebooted after the service had been enabled. No manual `systemctl start` or restart was issued after boot before inspection.

The service reported:

```text
enabled
active
MainPID=810
Result=success
NRestarts=0
ActiveEnterTimestamp=Tue 2026-09-01 20:02:16 PDT
InvocationID=a6d7da075daa45d98c33122aacccbf08
```

The current-boot journal contained only the automatic boot startup:

```text
Sep 01 20:02:16 debiancb1 systemd[1]: Starting blueroute.service - BlueRoute Bluetooth PAN networking daemon...
Sep 01 20:02:16 debiancb1 systemd[1]: Started blueroute.service - BlueRoute Bluetooth PAN networking daemon.
```

`busctl --system status org.blueroute.Service1` again showed the service owner in `system.slice`, not a user session.

## GUI logout survival

With the boot-started daemon still running, the graphical desktop session was logged out normally and then logged back in. No BlueRoute service action was issued during the logout/login cycle.

After login the service state was:

```text
active
MainPID=810
Result=success
NRestarts=0
ActiveEnterTimestamp=Tue 2026-09-01 20:02:16 PDT
InvocationID=a6d7da075daa45d98c33122aacccbf08
```

Both `MainPID` and `InvocationID` are identical to the values captured before GUI logout, and `NRestarts` remained zero. The journal contained no restart entry after the original boot start. Therefore the same daemon process survived destruction and recreation of the graphical user session.

## Acceptance result

**PASS.** On the tested Debian 13 baseline, the shipped P5-006 service:

- is enabled as a system service and starts automatically at boot;
- does not depend on a graphical or per-user systemd session;
- uses D-Bus ownership as readiness (`Type=dbus`);
- owns `org.blueroute.Service1` through a narrowly scoped root ownership policy;
- creates `/var/lib/blueroute` and `/run/blueroute` with mode `0700` and root ownership;
- records startup/failure/restart state in journald;
- restarts after unexpected daemon failure according to `Restart=on-failure`;
- survives GUI logout/login without process replacement or restart.

This hardware result is specific to the tested Debian/systemd environment and is not a portability claim for every Linux distribution or init system.

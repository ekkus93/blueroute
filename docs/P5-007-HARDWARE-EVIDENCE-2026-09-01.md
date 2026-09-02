# P5-007 Hardware Acceptance — 2026-09-01

## Scope

This record closes the physical Debian acceptance for **P5-007 — Define D-Bus/Polkit authorization policy**. The test validates that ordinary local users can inspect BlueRoute status without elevation, that mutating operations cross the PolicyKit boundary using the original D-Bus caller identity, that an unauthorized local process is denied before mutation dispatch, and that denial does not destabilize the daemon.

The test does not claim that every future mutation is implemented. At P5-007, the security requirement is that authorization is enforced before dispatch; command-specific behavior is implemented by the later product tasks that own those operations.

## Test system

- Host: `debiancb1`
- Distribution: Debian GNU/Linux 13 (trixie)
- Kernel: `6.12.86+deb13-amd64`
- BlueZ: `5.82`
- NetworkManager: `1.52.1`
- systemd: `257 (257.9-1~deb13u1)`
- P5-007 branch: `P5-007_dbus_polkit_authorization`
- Final live acceptance candidate: `d460c14651dd893d60e54f7d276db0df30ae6adb`
- Service: `org.blueroute.Service1`
- Object: `/org/blueroute/Service1`
- Interface: `org.blueroute.Service1`
- PolicyKit actions: `org.blueroute.modify`, `org.blueroute.internet-sharing`

## Installed authorization policy

The candidate daemon, D-Bus policy, and PolicyKit action definitions were installed on the live Debian system and `blueroute.service` restarted successfully. The service reported:

```text
active
```

`pkaction --verbose` showed both BlueRoute actions registered with conservative defaults:

```text
org.blueroute.modify:
  description:        Modify BlueRoute networking
  message:            Authentication is required to modify BlueRoute networking
  implicit any:       no
  implicit inactive:  auth_admin
  implicit active:    auth_admin_keep

org.blueroute.internet-sharing:
  description:        Change BlueRoute Internet sharing
  message:            Authentication is required to change BlueRoute Internet sharing
  implicit any:       no
  implicit inactive:  auth_admin
  implicit active:    auth_admin_keep
```

The system-bus policy continues to restrict ownership of the well-known BlueRoute service name to `root` while allowing local callers to send requests to that service. Method/command authorization is therefore enforced by the daemon rather than by granting front ends root privileges.

## Normal-user read-only access

As the ordinary `debian` login user, with no `sudo` and no authentication prompt, the live system-bus service accepted read-only inspection:

```text
Version
=> qq 1 0

Status
=> successful serialized status response
```

The returned status described the live daemon, including API version 1.0, the persisted local node identity, healthy state, and discovered capabilities. This proves intended users can inspect daemon state without crossing the privileged mutation policy.

## Unauthorized mutation denial

A separate request was deliberately issued as the unprivileged `nobody` account:

```text
sudo -u nobody busctl --system call \
  org.blueroute.Service1 \
  /org/blueroute/Service1 \
  org.blueroute.Service1 \
  Request \
  s \
  '{"type":"create_network","data":{"name":"Unauthorized probe"}}'
```

The live daemon rejected it:

```text
Call failed: Access denied
busctl exit=1
```

This is the required fail-closed result. The request did **not** return the daemon skeleton's `NotSupported` result, which means the unauthorized command did not pass through authorization into mutation dispatch.

After the denial:

```text
systemctl is-active blueroute.service
=> active
```

The authorization failure therefore did not crash, restart, or otherwise destabilize the daemon.

## CI authorization-path coverage

The hardware test proves the real Debian D-Bus/PolicyKit denial boundary and ordinary-user read-only behavior. The deterministic real-broker CI test supplements that evidence with a mock PolicyKit authority and proves:

- read-only commands do not contact PolicyKit;
- malformed commands fail before authorization;
- denied mutations return `AccessDenied`;
- the PolicyKit subject is the original `system-bus-name` caller;
- normal mutations use `org.blueroute.modify`;
- reserved Internet-sharing mutation uses `org.blueroute.internet-sharing`;
- an authorized mutation crosses the authorization boundary and reaches the current command handler;
- PolicyKit absence/error fails closed rather than falling back to caller UID assumptions, shell helpers, or direct front-end privilege.

CI run `33587964786` passed the formatting, locked workspace check, Clippy, test suite, real-broker daemon/client tests, authorization integration test, systemd validation, and D-Bus/PolicyKit packaging validation on candidate `d460c14651dd893d60e54f7d276db0df30ae6adb`.

## Acceptance result

**PASS.** On the tested Debian 13 baseline:

- read-only BlueRoute inspection is available to the normal unprivileged user without authentication;
- mutating operations are explicitly classified and routed through PolicyKit;
- normal mutations and Internet-sharing use separate action IDs;
- the original D-Bus sender identity is used as the PolicyKit subject;
- an arbitrary unprivileged local process cannot perform an unprotected BlueRoute mutation;
- denial occurs before mutation dispatch and fails closed;
- PolicyKit unavailability/errors also fail closed in deterministic acceptance coverage;
- front ends do not need blanket root access;
- authorization denial leaves the long-running system daemon healthy.

This evidence is specific to the tested Debian/PolicyKit/system-D-Bus environment and does not imply identical policy-agent behavior on every Linux distribution or desktop environment.

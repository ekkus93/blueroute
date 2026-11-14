# CR-F5: Removal Cleanup Design

Status: accepted (2026-11-13); **revised 2026-11-13 by FU-04** (see "Joined-client networks"): the original skip-and-succeed rule for an unobservable PANU link conflicted with CLEAN-5 and is withdrawn. Requirements: CLEAN-1–CLEAN-7 in
[CODE_REVIEW_REMEDIATION_SPEC.md](CODE_REVIEW_REMEDIATION_SPEC.md). This records CR-F5-01.

## What can remain after the service stops

| Resource | Created by | After `systemctl stop blueroute` |
| --- | --- | --- |
| BlueZ NAP registration (`NetworkServer1.Register`) | host start | Gone: BlueZ unregisters it when the daemon's D-Bus connection closes. |
| BlueZ advertisement | host start | Gone, for the same reason. |
| PANU link of a joined client (`Network1.Connect`) | join/reconnect | Expected gone (BlueZ watches the caller), but not assumed: cleanup still disconnects a live PANU link it can prove is ours. |
| NetworkManager bridge profile and interface | host start | **Remains**, with BlueRoute ownership metadata (`user.data`, full NetworkId). |
| NetworkManager-owned IPv4 address | host start, join | **Remains**, carrying the NetworkId owner. |
| IP forwarding | nothing | The daemon never enables forwarding (routing is not implemented; P18 stays off). Cleanup must not touch forwarding. No lease exists to release. |
| Routes, NAT, firewall | nothing | Never created; cleanup adds no such behavior (CLEAN-3). |

So the stopped-service cleanup is NetworkManager state plus, for a joined client, a PANU link
that must be observed: proven ours and disconnected, or observed absent.

## Entry point (CLEAN-1, CLEAN-7)

`/usr/libexec/blueroute/blueroute-daemon --remove-owned-networking`: a fixed maintenance mode of
the daemon binary, using the same typed adapters and ownership code as leave. It takes no other
argument, runs no external program, and refuses unless it runs as root (checked through the owner
of `/proc/self`, without `unsafe`). The state directory is the daemon's (`BLUEROUTE_STATE_DIR` or
`/var/lib/blueroute`). It is not on the D-Bus API, so no front end gains a privilege.

## Writer quiescence (CLEAN-2)

- The daemon holds an exclusive advisory lock on `<state>/daemon.lock` for its whole life and
  refuses to start when it cannot take it.
- Cleanup takes the same lock without waiting. If a daemon still holds it, cleanup fails ("stop
  blueroute.service first") and changes nothing.
- Before touching anything, cleanup writes `<state>/removal-pending`. A daemon refuses to start
  while it exists, so nothing (manual start, reconnect, startup restoration, control worker) can
  recreate networking during or after cleanup. `postinst` deletes the marker on `configure`
  (reinstall, upgrade) and on `abort-remove`/`abort-deconfigure`, then starts the service.

## Scope (CLEAN-2, CLEAN-3)

- Known networks: every NetworkId in this installation's membership registry, plus the client
  record's network. For each, remove NetworkManager addresses and profiles whose ownership is
  that exact NetworkId (`leave_observed`), disconnecting first a live PANU link to the recorded
  host device when it runs on an interface carrying that ownership.
- Resources carrying BlueRoute ownership for a NetworkId this installation does not know (another
  installation, a purged one): preserved and reported as unresolved. They do not fail removal,
  because they are not this installation's.
- Anything without BlueRoute ownership metadata is never touched.

### Joined-client networks (FU-04, REMOVE-1..3)

A network is a joined-client network when its persisted membership role is `client`; the client
record only supplies the recorded host device (a record left behind by a leave is ignored).
Bluetooth being unreachable is not evidence that its PANU link is gone, so:

- With Bluetooth up and the host device recorded, the link to that device is disconnected only
  when it runs on an interface carrying this network's ownership, and then observed again; the
  owned address and profile (the proof) are removed only after it is observed gone. A link that
  is already absent counts as gone. A link to another device, or one on an interface without this
  network's ownership, is never touched.
- "No link observed" counts only when BlueZ has the recorded host device object; a missing
  object (no adapter, a stale or renumbered path) is inconclusive.
- Otherwise (Bluetooth unreachable, no recorded host device, or BlueZ without that device) the
  network is left completely untouched, counted as `unresolved_client_links`, and cleanup fails. A retry with Bluetooth
  back finds the proof still in place.
- Host networks never need Bluetooth.

No new durable cleanup record exists: the NetworkManager ownership metadata stays the proof
(maintainer decision, 2026-11-13).
- An unreadable membership registry fails cleanup: ownership cannot be proven.

## Order, bounds and failure (CLEAN-4, CLEAN-5)

1. Root check, lock, marker.
2. Load the registry and client record.
3. Connect NetworkManager (required when any network is known; with none known there is nothing
   this installation can own, so cleanup succeeds without it). Connect BlueZ best effort; without
   it, host networks are still cleaned and joined-client networks are left unresolved.
4. Per known network: PANU first (while the owned profile still proves it, then observed gone),
   then addresses, then profiles. One network's failure does not stop the others.
5. Reobserve. Success only if no address or profile owned by a known network remains, no step
   failed, and no joined-client link is unresolved.
6. The whole run is bounded by 60 seconds; a timeout is a failure.

Logs are JSON lines with categories, counts and coarse failure classes only. Exit status 0 is
success, 1 is failure (resources remain, a client link is unresolved, a backend was unreachable, timeout, a
running daemon),
2 is a refused invocation. A retry reobserves and resumes; every step is idempotent. Membership is
never cleared to hide a failure.

## Debian behavior (CLEAN-6)

| Script, action | Behavior |
| --- | --- |
| `prerm remove`, `prerm deconfigure` | Stop the service, then run the cleanup. A cleanup failure fails `prerm`, so dpkg keeps the package installed and runs `postinst abort-remove` (or `abort-deconfigure`). |
| `postinst abort-remove`, `abort-deconfigure` | Delete the marker, start the service again. |
| `prerm upgrade`, `failed-upgrade` | Nothing: the service keeps running and `postinst configure` restarts it on the new binary. Networking is not torn down for an upgrade. |
| `postinst configure` | Delete the marker, enable, start (fresh) or restart (upgrade). |
| `postrm remove` | Disable the unit. Durable state stays. |
| `postrm purge` | Delete `/var/lib/blueroute`. Purge of an installed package runs `prerm remove` first, and purge of a removed package follows a successful remove, so the state is deleted only after cleanup succeeded. |

Forced removal or manual administrator intervention after a failed cleanup is outside the success
guarantee. Physical qualification (no active networking after uninstall on real hardware) is
CR-QUALIFY.

# CR-QUALIFY: Installed-System and Physical Cleanup Qualification

Status: **NOT STARTED** (needs equipment). Task: CR-QUALIFY in
[CODE_REVIEW_REMEDIATION_TODO.md](CODE_REVIEW_REMEDIATION_TODO.md). The design under test is
[CR-F5-REMOVAL-CLEANUP.md](CR-F5-REMOVAL-CLEANUP.md). Software and isolated-dpkg results are in
[CODE_REVIEW_REMEDIATION_EVIDENCE.md](CODE_REVIEW_REMEDIATION_EVIDENCE.md); this procedure is what
those cannot show. Synthetic dpkg roots do not count here.

There are two parts, and each produces its own evidence record:

- **Part A — installed system (CR-QUALIFY-01..05):** a disposable VM or spare machine with real
  systemd, D-Bus, PolicyKit and NetworkManager. Needs no Bluetooth hardware.
- **Part B — Bluetooth rig (CR-QUALIFY-06..07):** a designated test rig with real adapters.

Never run this on a computer whose networking matters. Part A deletes and recreates NetworkManager
profiles and installs and removes system packages.

## Pass rules

A step passes only if what was **observed independently** (NetworkManager, the kernel, BlueZ,
dpkg) matches the expectation. BlueRoute's own output is recorded but is not proof. Record every
deviation, including ones that look harmless. Do not generalize one machine's result to other
adapters, distributions or NetworkManager versions.

## Inspection tools (test procedure only)

The product never shells out; this procedure may, to observe state independently. Save this
snapshot helper on the test machine (not in the package):

```bash
#!/usr/bin/env bash
# qa-snapshot.sh LABEL: record networking state to ./qa/LABEL/
set -euo pipefail
out="qa/$1"; mkdir -p "$out"
nmcli -t -f NAME,UUID,TYPE,DEVICE,ACTIVE connection show > "$out/nm-connections.txt"
for uuid in $(nmcli -t -f UUID connection show); do
    printf '%s %s\n' "$uuid" "$(nmcli -g user.data connection show "$uuid" 2>/dev/null)"
done > "$out/nm-user-data.txt"
ip -br link > "$out/links.txt"
ip -br -4 addr > "$out/addresses.txt"
ip -4 route show table all > "$out/routes.txt"
ip -4 rule > "$out/rules.txt"
sysctl -n net.ipv4.ip_forward > "$out/ip_forward.txt"
ls -la /var/lib/blueroute > "$out/state-dir.txt" 2>&1 || true
dpkg-query -W -f='${Package} ${Version} ${Status}\n' 'blueroute*' > "$out/dpkg.txt" 2>&1 || true
systemctl is-active blueroute.service > "$out/service.txt" 2>&1 || true
if busctl status org.bluez >/dev/null 2>&1; then
    busctl tree org.bluez > "$out/bluez-tree.txt"
fi
```

Compare two snapshots with `diff -r qa/BEFORE qa/AFTER`. A BlueRoute-owned profile is named
`blueroute-bridge-<first 8 hex of the NetworkId>`, its device is `brb-<same 8 hex>`, its address is
in `10.201.0.0/16`, and its `user.data` carries `org.blueroute.owner=<NetworkId>`,
`org.blueroute.kind` and `org.blueroute.schema=1`.

## Part A — installed system

### A0. Prepare (CR-QUALIFY-01)

1. A disposable Debian 13 VM or spare machine with NetworkManager managing networking. Take a
   snapshot to revert to between scenarios.
2. Check out the candidate revision and build the packages:
   `bash packaging/debian/build-package.sh`.
3. Record: source SHA, `sha256sum target/debian/*.deb`, `uname -a`, `/etc/os-release`,
   `NetworkManager --version`, `systemctl --version`, `dpkg --version`, BlueZ version (if
   installed), and whether a Bluetooth adapter exists (none is needed for Part A).
4. Install: `sudo apt install ./target/debian/blueroute_*.deb`. Confirm
   `systemctl is-active blueroute.service`.

A host network needs a powered Bluetooth adapter for BlueZ NAP. If the VM has none, use USB
passthrough of a test adapter, or run Part A on the rig host instead; record which.

### A1. Fixtures (CR-QUALIFY-02)

1. Foreign comparison state, created outside BlueRoute:
   - a plain bridge profile `qa-foreign-br` with address `10.250.0.1/24`;
   - an extra address on an existing interface, and a static route
     (`10.251.0.0/24 via` that interface's gateway or `dev` it);
   - an **unknown-owner** profile: a bridge `qa-other-install` whose `user.data` carries
     `org.blueroute.owner=` a NetworkId this installation does not know (32 hex characters),
     `org.blueroute.kind=bridge`, `org.blueroute.schema=1`. This stands for another installation.
     Set it with `nmcli connection modify qa-other-install user.data 'org.blueroute.owner=…,org.blueroute.kind=bridge,org.blueroute.schema=1'`
     and confirm with `nmcli -g user.data connection show qa-other-install`; if your nmcli
     version rejects that syntax, record how you set it.
2. Owned state: `blueroute network create "QA"`. Confirm one owned profile, device and address
   as described above.
3. `./qa-snapshot.sh A1-before`.

### A2. Remove while active (CR-QUALIFY-02, 04, 05)

1. `sudo apt remove blueroute 2>&1 | tee qa/A2-apt.log`.
2. `./qa-snapshot.sh A2-after`; diff against `A1-before`.

Expected:

- [ ] apt succeeded; `qa/A2-apt.log` (and `/var/log/apt/term.log`) contains one
  `"component":"removal_cleanup"` line with `"outcome":"succeeded"`, `"remaining_owned_resources":0`
  and `"unresolved_foreign_resources"` ≥ 1 (the unknown-owner profile).
- [ ] The owned profile, `brb-…` device and `10.201.x.x` address are gone.
- [ ] `qa-foreign-br`, `qa-other-install`, the extra address, the static route, the default route
  and every other profile are unchanged (byte-identical in the snapshot, apart from runtime
  counters).
- [ ] `ip_forward` is unchanged.
- [ ] `/var/lib/blueroute` still holds `node-id`, `memberships-v1` and keys, plus the
  `removal-pending` marker; the package is in `rc` state.
- [ ] Reinstall (`sudo apt install ./…deb`): the marker is gone, the service starts, `blueroute
  status` shows the same NodeId and the same network, and the host comes back (owned profile and
  address present again). This proves identity and membership survived removal.

### A3. Further removal cases (CR-QUALIFY-03)

Revert to the A1 snapshot (or recreate the fixtures) before each case.

- [ ] **Service already stopped:** `sudo systemctl stop blueroute.service`, then remove. Expected
  as in A2.
- [ ] **Bluetooth unavailable:** `sudo systemctl stop bluetooth.service`, then remove. Expected
  as in A2 with `"bluetooth_available":false`.
- [ ] **Backend failure, then recovery:** `sudo systemctl stop NetworkManager.service`, then remove.
  Expected: apt reports the removal failed; the package is still installed (`ii`); the log line
  says `"outcome":"failed"`; the service was started again (`abort-remove`); owned and foreign
  state are unchanged; the marker is absent. Start NetworkManager, wait for the host to be
  repaired (about 15 s), remove again: succeeds as in A2.
- [ ] **Repeated cleanup:** with the service stopped, run
  `sudo /usr/libexec/blueroute/blueroute-daemon --remove-owned-networking` twice. Both succeed
  and report `"remaining_owned_resources":0`; the second finds nothing left to remove. Then confirm the daemon refuses to start
  (`systemctl start` ends failed after its restart attempts; the journal says it is being removed), delete
  `/var/lib/blueroute/removal-pending`, and confirm it starts and restores the host.
- [ ] **Running daemon:** with the service running, run the same command: it fails ("stop
  blueroute.service first") and changes nothing.
- [ ] **Remove, then purge:** remove (as A2), then `sudo apt purge blueroute`: `/var/lib/blueroute`
  is gone; networking as after A2.
- [ ] **Direct purge:** from an installed, active state, `sudo apt purge blueroute`: cleanup runs
  first (log line present, succeeded), then the state directory is deleted.

### A4. Upgrade and restart do not run removal (CR-QUALIFY-04)

- [ ] Build a second package with a higher version (change the workspace version locally, do not
  commit), install it over the first. Expected: no `removal_cleanup` line, the service is
  restarted, the owned profile and address stay up the whole time (watch `ip -br addr` during the
  upgrade), identity and membership unchanged.
- [ ] `sudo systemctl restart blueroute.service`: no cleanup; the host stays or is restored.
- [ ] Reboot: no cleanup; the host is restored at boot.

### A5. Final state check (CR-QUALIFY-05)

After A2 and after each successful A3 case:

- [ ] no profile with `org.blueroute.owner` of a known network, no `brb-` device, no `10.201.`
  address, no route via such a device;
- [ ] the foreign fixtures, default route and `ip_forward` match the before snapshot. If you set
  `ip_forward=1` before installing, it must still be 1 afterwards (the daemon never writes it).

## Part B — Bluetooth rig

Two computers with adapters, Wi-Fi isolated so Bluetooth carries the checked traffic. Record both
machines' environment as in A0, plus controller model, firmware and BlueZ version.

### B1. Host teardown with a live PANU (CR-QUALIFY-06)

1. On the host, install and `blueroute network create "Rig"`.
2. Attach the second computer as a PANU client to the host's NAP, outside BlueRoute (the
   method used for P6-001's physical acceptance), and confirm traffic over `brb-…`.
3. Snapshot both machines, including the host's `busctl tree org.bluez` and its advertising
   instances (`busctl get-property org.bluez /org/bluez/hci0 org.bluez.LEAdvertisingManager1 ActiveInstances`).
4. Remove the package on the host.

Expected:

- [ ] the NAP registration and BlueRoute advertisement are gone (advertising instances back to the
  pre-install count), the client's PANU link drops, the host's owned profile and address are gone;
- [ ] pairings, trusted devices and any other Bluetooth or network state on both machines are
  unchanged.

If the rig has an `experimental-join` build, repeat with a BlueRoute-joined client and remove the
package **on the client**: the PANU link to the recorded host and the client's owned profile and
address are removed; a PANU link to any other device is left alone. Record whether BlueZ had
already dropped the link when the daemon stopped (the design expects it, but does not rely on it).

### B2. Recovery and live UI (CR-QUALIFY-07)

Only needed for claims actually being made:

- [ ] **Adapter recovery without restart (CR-F4):** power the adapter off, restart the daemon,
  confirm `blueroute capability show` does not report hosting and `network create` is refused.
  Power the adapter on; within about 15 s status reports hosting, and `network create` succeeds
  without restarting BlueRoute.
- [ ] **Connected desktop and TUI (CR-F1/F2):** with both open, create and leave a network from the
  CLI: both update within about a second. Stop the service: both say the view may be out of date
  within 2 s. Start it: both reconnect and refresh without user action.

## Evidence record (CR-QUALIFY-08)

Write one record per part, `docs/CR-QUALIFY-A-EVIDENCE-<date>.md` and
`docs/CR-QUALIFY-B-EVIDENCE-<date>.md`, and link them from the remediation TODO's CR-QUALIFY
section and ledger and from [HARDWARE_ACCEPTANCE_BACKLOG.md](HARDWARE_ACCEPTANCE_BACKLOG.md). Each
contains:

- the environment block from A0 (plus the Part B hardware details);
- for every checkbox above: pass, fail or not run, with the observation that decided it;
- the snapshot diffs and apt logs, redacted of MAC addresses and host names if they must leave the
  machine (keep NetworkIds; they are not secret);
- every deviation and unresolved case, each with a follow-up task or an explicit acceptance;
- what was **not** covered (other distributions, NetworkManager versions, adapters).

CR-QUALIFY can be ticked only when Part A passes and, for each physical claim made, Part B does.
If equipment is not available, the task stays open; do not substitute software evidence.

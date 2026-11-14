# CR-QUALIFY: Installed-System and Physical Cleanup Qualification

Status: **procedure ready, NOT EXECUTED** (needs equipment; revised 2026-11-13 by FU-06: Part A
no longer needs an adapter, and a seeded joined client must fail honestly). Task: CR-QUALIFY in
[CODE_REVIEW_REMEDIATION_TODO.md](CODE_REVIEW_REMEDIATION_TODO.md). The design under test is
[CR-F5-REMOVAL-CLEANUP.md](CR-F5-REMOVAL-CLEANUP.md). Software and isolated-dpkg results are in
[CODE_REVIEW_REMEDIATION_EVIDENCE.md](CODE_REVIEW_REMEDIATION_EVIDENCE.md); this procedure is what
those cannot show. Synthetic dpkg roots do not count here.

There are two parts, and each produces its own evidence record:

- **Part A — installed system (CR-QUALIFY-01..05):** a disposable VM or spare machine with real
  systemd, D-Bus, PolicyKit and NetworkManager. Needs **no Bluetooth adapter**: owned state comes
  from the test-only fixture program, not from `network create` (which needs a NAP adapter).
- **Part B — Bluetooth rig (CR-QUALIFY-06..07):** a designated test rig with real adapters.

Never run this on a computer whose networking matters. Part A deletes and recreates NetworkManager
profiles and installs and removes system packages.

## Evidence layers and isolation (FU-06, QUAL-2/3)

| Layer | What it proves | Where |
| --- | --- | --- |
| Deterministic software | Cleanup logic, report, exit status, fake backends | `apps/daemon/src/removal.rs` tests; CI |
| Isolated package harness | Real dpkg transitions and maintainer-script ordering with a **mocked** helper and mocked systemd helpers in a temporary dpkg root | `packaging/debian/test-package-upgrade.sh`; CI |
| Installed system (Part A) | The real helper against real NetworkManager, systemd, D-Bus and dpkg | this procedure, disposable VM |
| Physical (Part B) | Real BlueZ NAP/PANU teardown and recovery | this procedure, Bluetooth rig |

A result counts only for its layer. Isolation facts the evidence must state:

- The isolated package harness relocates **files** with `--root`/`DPKG_ROOT`; it does **not**
  isolate D-Bus. That is why it replaces the installed helper with a mock before any removal: the
  real helper would reach the host's system bus and NetworkManager. Never describe that harness as
  bus isolation.
- Unit and private-bus tests never reach host services. Part A and Part B deliberately use the
  real system services of a **disposable** machine; record which machine.
- The fixture program `apps/daemon/examples/qualify_fixture.rs` is test-only: it is not packaged,
  writes state with the production stores and adapter, and grants no ownership bypass. The helper
  decides ownership exactly as for real state.

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

## Part A — installed system (no Bluetooth adapter)

### A0. Prepare (CR-QUALIFY-01)

1. A disposable Debian 13 VM with NetworkManager managing networking, **no Bluetooth adapter**,
   and `bluez` installed only because the package depends on it. Snapshot it to revert between
   cases.
2. At the candidate revision: `bash packaging/debian/build-package.sh` and
   `cargo build --release --locked -p blueroute-daemon --example qualify_fixture`; copy
   `target/debian/blueroute_*.deb` and `target/release/examples/qualify_fixture` to the VM.
3. Record: source SHA, `sha256sum` of the package and fixture binary, `uname -a`,
   `/etc/os-release`, `NetworkManager --version`, `systemctl --version`, `dpkg --version`,
   `bluetoothd --version`, and that no adapter exists (`busctl tree org.bluez` shows no `hci`).
4. `sudo apt install ./blueroute_*.deb`; confirm `systemctl is-active blueroute.service`. With no
   adapter the daemon reports degraded health; that is expected.

### A1. Fixtures (CR-QUALIFY-02)

Stop the service first so the fixture writes the state directory alone:
`sudo systemctl stop blueroute.service`. Then, all as root:

1. **Foreign comparison state** (outside BlueRoute):
   - a plain bridge profile `qa-foreign-br` with address `10.250.0.1/24`;
   - an extra address on an existing interface and a static route `10.251.0.0/24` via it;
   - optionally `sysctl -w net.ipv4.ip_forward=1` to check that cleanup never resets it.
2. **Known hosted network:** `sudo ./qualify_fixture host /var/lib/blueroute` → prints NetworkId
   `H`. Expect profile `blueroute-bridge-<H[0..8]>`, device `brb-<H[0..8]>`, an address in
   `10.201.0.0/16`, `user.data` = `org.blueroute.owner=H`, `org.blueroute.kind=bridge`,
   `org.blueroute.schema=1`, and `H` in `/var/lib/blueroute/memberships-v1` with role `host`.
3. **Another installation's network:** `sudo ./qualify_fixture foreign /var/lib/blueroute` →
   NetworkId `F`, owned bridge and address like the above, but **not** in the registry.
4. Fix permissions if the fixture created the state directory: `chmod 700 /var/lib/blueroute`.
5. `sudo systemctl start blueroute.service` (it restores `H` as a host only if an adapter existed;
   here it reports the failed restore and keeps the owned NetworkManager state), then
   `./qa-snapshot.sh A1-before`.

### A2. Remove (CR-QUALIFY-02, 04, 05)

1. `sudo apt remove blueroute 2>&1 | tee qa/A2-apt.log`, then `./qa-snapshot.sh A2-after`.

Expected:

- [ ] apt succeeded; one `"component":"removal_cleanup"` line (in the apt output and
  `/var/log/apt/term.log`) with `"outcome":"succeeded"`, `"remaining_owned_resources":0`,
  `"unresolved_client_links":0`, `"unresolved_foreign_resources"` ≥ 2 (`F`'s profile and address),
  `"bluetooth_available":false`.
- [ ] `H`'s profile, `brb-` device and address are gone.
- [ ] `F`'s bridge and address, `qa-foreign-br`, the extra address, the static route, the default
  route and every other profile are unchanged; `ip_forward` is unchanged.
- [ ] `/var/lib/blueroute` still holds `node-id`, `memberships-v1` and keys, plus `removal-pending`;
  dpkg shows `rc`.
- [ ] Reinstall: the marker is gone, the service starts, the same NodeId and network `H` are
  reported (identity and membership survived).

### A3. Further removal cases (CR-QUALIFY-03)

Revert to the A1 snapshot before each case.

- [ ] **Service already stopped:** stop it, then remove. Expected as A2.
- [ ] **Seeded joined client, no Bluetooth (QUAL-2):** create a dummy interface
  (`ip link add qa-bnep0 type dummy && ip link set qa-bnep0 up`), then
  `sudo ./qualify_fixture client /var/lib/blueroute qa-bnep0` → NetworkId `C`. Remove. Expected:
  **removal fails** (package stays `ii`), log line `"unresolved_client_links":1`; `C`'s owned
  address and profile on `qa-bnep0` remain (proof for a retry); `H` is still removed;
  `abort-remove` restarted the service. This proves the honest failure on a real system. It is
  **not** client PAN teardown, which only Part B can show. Teardown: revert the snapshot.
- [ ] **NetworkManager down, then recovery:** `systemctl stop NetworkManager`, remove: fails,
  package `ii`, `"outcome":"failed"`, marker absent, service restarted. Start NetworkManager,
  remove again: succeeds as A2.
- [ ] **Repeated cleanup:** service stopped, run
  `sudo /usr/libexec/blueroute/blueroute-daemon --remove-owned-networking` twice: both exit 0 and
  log `"remaining_owned_resources":0`. The daemon then refuses to start; delete
  `/var/lib/blueroute/removal-pending` and it starts again.
- [ ] **Running daemon:** run the same command with the service running: exit 1 ("stop
  blueroute.service first"), nothing changed, no marker written.
- [ ] **Remove, then purge:** as A2, then `sudo apt purge blueroute`: state directory gone.
- [ ] **Direct purge:** from installed, `sudo apt purge blueroute`: cleanup line first (succeeded),
  then the state directory is deleted.
- [ ] **Purge after an unresolved removal:** with the client fixture, `apt purge` fails at `prerm`
  exactly like remove; the state directory (the retry proof) is kept.

### A4. Upgrade and restart do not run removal (CR-QUALIFY-04)

- [ ] Install a second build with a higher version (local version bump, not committed): no
  `removal_cleanup` line, service restarted, `H`'s owned state untouched throughout, identity and
  membership unchanged.
- [ ] `systemctl restart blueroute.service` and a reboot: no cleanup, owned state untouched.

### A5. Final state check (CR-QUALIFY-05)

After A2 and each successful A3 case:

- [ ] no profile owned by a known NetworkId, no `brb-` device of a known network, no `10.201.`
  address on one, no route via one;
- [ ] foreign fixtures (including `F`), default route and `ip_forward` match the before snapshot.

## Part B — Bluetooth rig

Two computers with adapters, Wi-Fi isolated so Bluetooth carries the checked traffic. Record both
machines' environment as in A0, plus controller model, firmware and BlueZ version.

### B1. Host teardown with a live PANU (CR-QUALIFY-06)

Part B is the only place where real PAN teardown is shown: removing a NAP registration and
advertisement, and disconnecting a client's PANU link.

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
package **on the client**:

- [ ] with `bluetooth.service` stopped: removal fails with `"unresolved_client_links":1`, the
  client's owned profile and address stay;
- [ ] start `bluetooth.service`, remove again: the PANU link to the recorded host is disconnected
  (or found already gone), then the owned profile and address are removed; removal succeeds;
- [ ] a PANU link to any other device is left alone throughout;
- [ ] record whether BlueZ had already dropped the link when the daemon stopped (the design does
  not rely on it).

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

- the environment block from A0 (plus the Part B hardware details), including the fixture
  binary's hash and whether each step used the fixture, a real network, or a mock;
- for every checkbox above: pass, fail or not run, with the observation that decided it;
- the snapshot diffs and apt logs, redacted of MAC addresses and host names if they must leave the
  machine (keep NetworkIds; they are not secret);
- every deviation and unresolved case, each with a follow-up task or an explicit acceptance;
- what was **not** covered (other distributions, NetworkManager versions, adapters).

CR-QUALIFY can be ticked only when Part A passes and, for each physical claim made, Part B does.
If equipment is not available, the task stays open; do not substitute software evidence.

# CR-QUALIFY Part A evidence, rerun (2026-11-17)

**Result: Part A PASSED** on candidate `fc93e98`. Every checkbox of Part A in
[CR-QUALIFY-PROCEDURE.md](CR-QUALIFY-PROCEDURE.md) (as revised at that SHA) was observed to hold,
except the "with `bluetoothd` running" variants, which cannot run without an adapter and belong to
Part B (QA-A-07). Part B was not run, so CR-QUALIFY stays open.

This rerun follows the [first run](CR-QUALIFY-A-EVIDENCE-2026-11-17.md), which failed on QA-A-01.
Fixes since then: `8dcf85a` (QA-A-01 service enablement across remove and reinstall, QA-A-03
systemd reload), `d31a7b5` (QA-A-02 cleanup refusal message), `fc93e98` (procedure and
`ADMINISTRATOR.md`, QA-A-04..09). This record also serves FU-QUALIFY-01, LC-QUALIFY-02 and
QR-QUALIFY-02 (it includes the seeded-client, unknown-role and recovery cases).

## Environment (A0)

| Item | Value |
| --- | --- |
| Source SHA | `fc93e9820df399b75303d5d30b37242dfea194cd` (master); CI run 44188257738 |
| Package | `blueroute_0.1.0_amd64.deb` sha256 `989a3ec3656cc4396dc9d45ab67d92fec724450a0afba4f9337beca88068f912`, built by `packaging/debian/build-package.sh` at the SHA in a clean worktree |
| Fixture | `qualify_fixture` sha256 `acbfa53a3f82cdccbac4877e13a341373939f39f795cb541ab1675d4110d8ca1` (test-only, not packaged) |
| Upgrade package (A4) | `blueroute_0.1.1_amd64.deb` sha256 `f99058f9f0fe6572b57be21f054ba0a6ce76bfb3abe1480ceabb6856a9849772`: same SHA, version bumped to 0.1.1 locally (Cargo.toml and Cargo.lock only, never committed) |
| Snapshot helper | `qa-snapshot.sh` extracted verbatim from the procedure at the SHA (ownership read over D-Bus) |
| Package checks before the run | On the built `.deb`: maintainer lifecycle, the new real-`deb-systemd-helper` enablement test and the isolated dpkg upgrade test all passed; `qualify_fixture` is not in the package |
| Machine | The same disposable QEMU/KVM VM as the first run, reverted to its clean snapshot (fresh Debian 13.7 generic cloud image, nothing installed): 2 vCPUs, 4 GiB, QEMU user-mode NAT only, no USB or Bluetooth device |
| Software | Kernel `6.12.117+deb13-amd64`; NetworkManager 1.52.1; systemd 257 (257.13-1~deb13u1); dpkg 1.22.22; `bluetoothd` 5.82 with **no adapter** (`/sys/class/bluetooth` absent, `org.bluez` not on the bus) |
| Run time | 2026-11-17 22:20–22:24 UTC (guest clock NTP-synchronized; resynchronized after every revert) |

NetworkIds: `H` = `27ce76d9cdaa1677c8b06c71bf577428` (hosted), `F` =
`914d5cd0c885845989f24b649e8d83b9` (another installation's),
`C` = `4e3c2ced48003e2e5e15e7a9c91c0857` (case 2) and `96f41aa8e9d9c073389084ad911c96fc`
(purge case), `U` = `5111613031a2355b8bb7555e656a2fd3`, `L` = `b2bb34bab675f5929f26eee688c10c2b`.
NodeId `fefb11287646bbb289d5739d728f709b`.

Owned state came from the fixture; no step used a real Bluetooth network or a mock. The client
stand-in `qa-bnep0` is an `ifb` link, as the procedure now says. Observations come from
NetworkManager (D-Bus `GetSettings`, `nmcli`), the kernel (`ip`, `sysctl`), dpkg and systemd.

## Results

| Step | Result | Deciding observation |
| --- | --- | --- |
| A0 install | PASS | `ii blueroute 0.1.0`; service `enabled` and `active`; `Health: Error` (no adapter, as the procedure expects) |
| A1 fixtures | PASS | `H`'s and `F`'s bridge profiles carry `org.blueroute.owner`, `kind=bridge`, `schema=1` (procedure helper); `brb-` devices with `10.201.217.1/24` and `10.201.208.1/24`; `H` in `memberships-v1` with role `host`, `F` absent; foreign bridge, extra address, static route and `ip_forward=1` present |
| A2 apt succeeded, one cleanup line | PASS | rc 0; one `removal_cleanup` line in apt output and `term.log`: `succeeded`, remaining 0, client links 0, foreign 2; `bluetooth_available` false |
| A2 `H` removed | PASS | `H`'s profile, `brb-27ce76d9`, address and routes gone |
| A2 foreign state unchanged | PASS | `F`, `qa-foreign-br`, `10.252.0.10/24`, route `10.251.0.0/24`, default route and `ip_forward` identical to A1-before |
| A2 state kept, `rc` | PASS | `node-id`, `memberships-v1`, `control-tls` identity and `removal-pending` present; dpkg `rc`; unit masked (`/etc/systemd/system/blueroute.service → /dev/null`) with its `multi-user.target.wants` link kept |
| A2 reinstall | PASS (QA-A-01 fixed) | Marker gone; `is-enabled` = enabled, `is-active` = active; NodeId `fefb1128…` equals `node-id`; network `H` listed |
| A3 service already stopped | PASS | As A2 |
| A3 seeded joined client | PASS | rc 100, `ii`; `"unresolved_client_links":1`; `C`'s profile and `10.201.237.2/24` on `qa-bnep0` kept; `H` removed; `F` untouched; journal shows `abort-remove`'s start and the expected two-network refusal |
| A3 NetworkManager down, then up | PASS | NM down: rc 100, `ii`, `"outcome":"failed"`, stderr "NetworkManager is not available on the system D-Bus", no marker, service active. NM up: as A2 |
| A3 repeated cleanup | PASS | Two runs rc 0, both remaining 0; daemon refused to start (`BlueRoute is being removed`); started after the marker was deleted |
| A3 running daemon | PASS (QA-A-02 fixed) | rc 1; stderr "a BlueRoute daemon or cleanup is running; stop blueroute.service first"; snapshots identical; no marker |
| A3 remove, then purge | PASS | State directory gone; no enablement link, mask or deb-systemd-helper record left; foreign state unchanged |
| A3 direct purge | PASS | Cleanup line (`succeeded`) before `Purging configuration files`; state directory, link, mask and records gone |
| A3 purge after unresolved removal | PASS | rc 100 at `prerm`; state directory and `C`'s profile kept; `H` removed |
| A3 unknown active role | PASS | Remove (bluetoothd stopped), remove (start attempted, skipped by systemd), purge: each rc 100, `"unresolved_roles":1`, `ii`; `U`'s profile, `10.201.48.2/24`, `member … none` line and client record kept; `H` removed; `F` untouched |
| A3 unknown role, recovery | PASS | `ADMINISTRATOR.md` lookup found exactly `U`'s profile (`Settings/6`), deleted by path; `U`'s `network` line and its client record deleted. Helper twice: rc 0 and 0, `"unresolved_roles":0`, `unresolved_foreign_resources` 2 as before recovery; then `apt remove` rc 0 |
| A3 completed leave, stale record | PASS | rc 0, `"unresolved_roles":0`; `L`'s profile, `10.201.186.2/24` and route gone; `F` untouched |
| A4 upgrade | PASS (QA-A-03 fixed) | No `removal_cleanup` line; no systemd "changed on disk" warning; service restarted at 22:23:13, `enabled`; owned state identical; `node-id` and `memberships-v1` byte-identical |
| A4 restart and reboot | PASS | No cleanup line; after the reboot the service is `enabled` and `active`, owned profiles unchanged (same UUIDs and `user.data`, keyfile mtimes from A1) and inactive as the procedure expects; identity and membership byte-identical |
| A5 final state | PASS | After A2 and each successful A3 case: no profile, `brb-` device, `10.201.` address or route of a known network; `F`, the foreign fixtures, the default route and `ip_forward` match the before snapshot |

## Deviations

- The "with `bluetoothd` running" variants were not run: `bluetooth.service` cannot start without
  an adapter (QA-A-07). They remain with Part B.
- QA-A-10 (an owned profile left when activation fails) is a known limitation of the join path:
  `join_network.rs` documents that IP setup can partially install owned state before failing and
  keeps joins blocked until P6-009 reconciliation; production join is gated off. The fixture is
  the only caller that can leave such a profile, and only on a failed setup.

No other deviation was observed. Not covered: other distributions, other NetworkManager or systemd
versions, real Bluetooth adapters and PAN links (Part B).

Raw evidence (`qa-evidence-*.tar.gz` per case and every command's output) was kept on the test
workstation, not committed; it contains only the VM's synthetic host name and virtual MAC
addresses.

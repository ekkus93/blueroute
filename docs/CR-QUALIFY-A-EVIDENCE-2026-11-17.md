# CR-QUALIFY Part A evidence (2026-11-17)

**Result: Part A FAILED on one product defect (QA-A-01, reinstall after remove leaves the service
disabled and stopped).** Every removal-cleanup expectation held: owned state was removed or kept
exactly as the procedure says, foreign state was never touched, and every failure was honest.
Seven further deviations are recorded below: two minor product findings, four procedure or
runbook defects and one environment limit. Part A must be rerun on a candidate that fixes QA-A-01.

Procedure: [CR-QUALIFY-PROCEDURE.md](CR-QUALIFY-PROCEDURE.md), Part A (A0–A5). Task: CR-QUALIFY in
[CODE_REVIEW_REMEDIATION_TODO.md](CODE_REVIEW_REMEDIATION_TODO.md). Part B was not run.

## Environment (A0)

| Item | Value |
| --- | --- |
| Source SHA | `3cc06a74dacd2b5c415e9b099f1e8236d2e01c0b` (master) |
| Package | `blueroute_0.1.0_amd64.deb` sha256 `02054dc6bd8408ecab58918a3c95d9e4186e750bd4c0b0bf19e40ec4722057e6`, built by `packaging/debian/build-package.sh` at the SHA in a clean worktree |
| Fixture | `qualify_fixture` sha256 `60c5c8256e9f5621492d4276476b3d13a615435dbd0253a013ddee6c19cb6d26` (`cargo build --release --locked -p blueroute-daemon --example qualify_fixture`; not packaged) |
| Upgrade package (A4) | `blueroute_0.1.1_amd64.deb` sha256 `49f7ed2d44599a7ec68dcb3e2c7088ed60c4fa64673a22555584b7232f9f00aa`: same SHA, workspace version bumped to 0.1.1 locally (Cargo.toml and Cargo.lock only, never committed) |
| Helper inside the package | `/usr/libexec/blueroute/blueroute-daemon` sha256 `1b4e4228e1f469dccb047e6cfdf940b805ec80f83654f9e2d0c2d79a9c47f664` |
| Toolchain | rustc 1.95.0 (59807616e 2026-04-14) |
| Machine | Disposable QEMU/KVM VM on the developer workstation: Debian 13.7 generic cloud image (`debian-13-generic-amd64.qcow2`, SHA-512 verified against Debian's `SHA512SUMS`), 2 vCPUs, 4 GiB. QEMU user-mode NAT only; the host's networking was not changed. No USB or Bluetooth device passed through. Live snapshots (`savevm`/`loadvm`) for reverts. |
| Kernel | `Linux blueroute-qa 6.12.117+deb13-amd64 #1 SMP PREEMPT_DYNAMIC Debian 6.12.117-1 (2026-11-12) x86_64` |
| NetworkManager | 1.52.1 (manages `enp0s2` with its own profile; netplan and systemd-networkd removed) |
| systemd / dpkg | systemd 257 (257.13-1~deb13u1); dpkg 1.22.22 |
| BlueZ | `bluetoothd` 5.82 installed; **no adapter**: `/sys/class/bluetooth` absent, `busctl tree org.bluez` fails (`org.bluez` cannot be activated) |
| Run time | 2026-11-17 21:42–21:51 UTC (guest clock NTP-synchronized; resynchronized after every revert) |

NetworkIds (not secret): `H` = `df581cb882bb01c44e2d8c0b1ace90b9` (hosted), `F` =
`76b5d98f2a0a4ecc1ab10060257f7bdd` (another installation's),
`C` = `967887c50471832d527e786f70cf4a6b` (case 2) and `e3325161322b8db91efe6883a3db0010` (case 8),
`U` = `7ae6b17e674d928a5af38e8a1ea52141`, `L` = `a717a8618beba08f1f0cf811aae08990`.
NodeId `e0a328f653717cfbf8326850caa4a447`.

Every owned state in Part A came from the fixture (production stores and adapter, no ownership
bypass). No step used a real Bluetooth network or a mock. Observations come from NetworkManager
(D-Bus `GetSettings`, `nmcli`), the kernel (`ip`, `sysctl`), dpkg and systemd; BlueRoute's own
cleanup line was recorded but not used as proof.

## Results

| Step | Result | Deciding observation |
| --- | --- | --- |
| A0 install | PASS (with QA-A-08) | `ii blueroute 0.1.0`; `blueroute.service` active |
| A1 fixtures | PASS | Profiles `blueroute-bridge-df581cb8`/`-76b5d98f` with `org.blueroute.owner`, `kind=bridge`, `schema=1` (read over D-Bus, QA-A-04); `brb-` devices with `10.201.184.1/24` and `10.201.143.1/24`; `H` in `memberships-v1` as `member … host`; `F` absent from it; foreign bridge, extra address, static route and `ip_forward=1` present |
| A2 apt succeeded, one cleanup line | PASS | apt rc 0; one `removal_cleanup` line in the apt output and `term.log`: `succeeded`, `remaining_owned_resources` 0, `unresolved_client_links` 0, `unresolved_foreign_resources` 2; `bluetooth_available` **false** (bluetoothd cannot run without an adapter) |
| A2 `H` removed | PASS | Snapshot diff: `H`'s profile, `brb-df581cb8`, its address and routes gone |
| A2 foreign state unchanged | PASS | `F`'s profile, device and address, `qa-foreign-br`, `10.252.0.10/24`, route `10.251.0.0/24`, default route and `ip_forward` identical to A1-before |
| A2 state kept, `rc` | PASS | `node-id`, `memberships-v1`, `control-tls/control-tls-identity-v1` present, plus `removal-pending`; dpkg `rc` |
| A2 reinstall | **FAIL (QA-A-01)** | Marker removed, but postinst printed `blueroute.service is a disabled or a static unit not running, not starting it.`; `is-enabled` = disabled. After a manual `systemctl start`: same NodeId, network `H` listed |
| A3 service already stopped | PASS | Same as A2 |
| A3 seeded joined client | PASS (with QA-A-05, QA-A-06, QA-A-07) | apt rc 100, package `ii`; `"unresolved_client_links":1`, `"outcome":"failed"`; `C`'s profile and `10.201.197.2/24` on `qa-bnep0` kept; `H` removed; `F` untouched. Run with bluetoothd stopped only |
| A3 NetworkManager down, then up | PASS | NM stopped: apt rc 100, `ii`, `{"failure_class":"NetworkBackendUnavailable","outcome":"failed"}`, no marker, service restarted and active. NM started: removal as A2 |
| A3 repeated cleanup | PASS | Service stopped: two runs, rc 0 and 0, both `remaining_owned_resources` 0; the daemon then refused to start (`BlueRoute is being removed`); after deleting `removal-pending` it started |
| A3 running daemon | PASS (with QA-A-02) | rc 1; snapshot before/after identical; no marker |
| A3 remove, then purge | PASS | Purge rc 0; `/var/lib/blueroute` gone; foreign state unchanged |
| A3 direct purge | PASS | Cleanup line (`succeeded`) printed before `Purging configuration files`; state directory gone |
| A3 purge after unresolved removal | PASS (with QA-A-05, QA-A-06) | Purge rc 100 at `prerm`, same line as remove; state directory and `C`'s profile and address kept |
| A3 unknown active role | PASS (with QA-A-05, QA-A-06, QA-A-07) | Remove (bluetoothd stopped), remove (after a start attempt that systemd skipped), and purge: each rc 100, `"unresolved_roles":1`, package `ii`; `U`'s profile and `10.201.126.2/24` kept; `member … none` line and client record kept; `H` removed; `F` untouched |
| A3 unknown role, recovery | PASS (with QA-A-04, QA-A-09) | Manual steps below; retry rc 0, `"unresolved_roles":0`, `unresolved_foreign_resources` 2 before and after; repeat rc 0, no change |
| A3 completed leave, stale record | PASS (with QA-A-05) | rc 0, `"unresolved_roles":0`; `L`'s profile, `10.201.97.2/24` and its route gone; `F` untouched |
| A4 upgrade | PASS (with QA-A-03) | No `removal_cleanup` line in apt output or new `term.log` lines; service stopped and started at 21:48:14; owned state identical; `node-id` and `memberships-v1` byte-identical |
| A4 restart and reboot | PASS | No cleanup line; owned profiles kept with the same UUIDs, `user.data` and keyfile mtimes. After the reboot the owned profiles are inactive because they are `autoconnect=false` and no adapter exists to restore `H` (A1.5 expects this); the extra address, static route and `ip_forward=1` were transient and are cleared by any reboot |
| A5 final state | PASS | After A2 and each successful A3 case: no profile of a known NetworkId, no `brb-` device, `10.201.` address or route of one; `F`, the foreign fixtures, the default route and `ip_forward` match the case's before snapshot |

### Recovery steps run (A3 unknown role)

1. `systemctl stop blueroute.service`. Full NetworkId `U` read from its `network` line and from the
   client record's first field; both name `U`.
2. No PAN link: no `bnep` interface, no adapter; `qa-bnep0` is an `ifb` stand-in.
3. Profiles with `org.blueroute.owner=U` exactly, read over D-Bus: one,
   `blueroute-interface-7ae6b17e` (`a25a01c9-0e62-5a60-b423-ae0b2533603d`); deleted with
   `nmcli connection delete uuid …`. Afterwards no profile names `U`, and `qa-bnep0` has no
   address.
4. Deleted `U`'s `network` line from `memberships-v1` (no `peer` lines existed; mode 600 kept) and
   `client-record/client-record-v1` after checking it names `U`.
5. `apt-get remove` succeeded; the repeat used the helper extracted from the same `.deb`
   (QA-A-09).

## Findings and deviations

| ID | Kind | Finding | Follow-up |
| --- | --- | --- | --- |
| QA-A-01 | **Product defect** (packaging) | `postrm remove` runs `deb-systemd-helper disable`. That deletes the `multi-user.target.wants` link but keeps `/var/lib/systemd/deb-systemd-helper-enabled/blueroute.service.dsh-also` (only purge deletes it). On reinstall, `postinst` runs `deb-systemd-helper enable`; because the `.dsh-also` file exists, `enable` treats the install as an upgrade (`debian_installed`), and because no link exists (`no_link_installed`) it treats the unit as disabled by the administrator, so it records state and creates no link. `deb-systemd-invoke start` then refuses the disabled unit. Reproduced twice from A1 with `_DEB_SYSTEMD_HELPER_DEBUG=1`. Every remove-then-reinstall leaves BlueRoute disabled and stopped. Debhelper's standard `postrm` does not disable on remove. The isolated package harness mocks the systemd helpers, so it cannot see this. | Fix the maintainer scripts' enable/disable handling, add a regression check, rerun Part A |
| QA-A-02 | Product (diagnostics, minor) | With the service running, `--remove-owned-networking` exits 1 correctly, but prints only `{"failure_class":"InvalidState","outcome":"failed"}` and the generic retry sentence. The instance-lock message "a BlueRoute daemon or cleanup is running; stop blueroute.service first" (`apps/daemon/src/removal.rs`) never reaches the operator. | Print the error message in maintenance mode |
| QA-A-03 | Product (packaging, minor) | After the upgrade, systemd warned `The unit file … changed on disk. Run 'systemctl daemon-reload'`. `postinst` never reloads systemd, so an upgrade that changes the unit would restart the service under the old definition. | Reload systemd in `postinst` before restarting |
| QA-A-04 | Procedure defect | `nmcli` 1.52 cannot display `user.data` (`Error: 'connection show': invalid field 'user.data'`). The procedure's `qa-snapshot.sh` therefore wrote empty ownership data, and `ADMINISTRATOR.md` step 5 tells administrators to use the same command. For this run the snapshot helper read each profile's `user` setting from NetworkManager over D-Bus (`Settings.Connection.GetSettings`) instead; the keyfiles agreed. | Update the procedure's helper and `ADMINISTRATOR.md` |
| QA-A-05 | Procedure defect | The procedure's `dummy` `qa-bnep0` cannot carry the client profile: the adapter writes `type=generic`, and NetworkManager 1.52 refuses it on a `dummy` device (`connection type is not "dummy"`). The first case 2 attempt therefore failed in the fixture and left an owned interface profile with no membership (removal then counted it as foreign). All client, unknown and left cases were run with `ip link add qa-bnep0 type ifb`, which NetworkManager classes as a generic device (DeviceType 14) and which activated the profile. Whether a real `bnep` device accepts the generic profile is a Part B question. | Update the procedure; check the profile type on a real `bnep` in Part B |
| QA-A-06 | Procedure defect | The client and unknown fixtures add a second `member` network to A1's `H`. The daemon refuses that state at startup (`durable state contains more than one active BlueRoute network`, `single_star::current_network`), so after a failed removal "`abort-remove` restarted the service" cannot be observed: the journal shows `abort-remove` started the service, and the daemon exited 1 and kept restarting. The removal checks themselves are unaffected. | Make the fixture refuse a second active network, or change the setup and the expectation |
| QA-A-07 | Environment limit | `bluetooth.service` has `ConditionPathIsDirectory=/sys/class/bluetooth`, so without an adapter systemd skips it; the "with `bluetoothd` running" variants of the client and unknown-role cases could not run. Only the stopped variants were run (for the unknown role, a start was attempted and skipped). | Run these variants on the Part B rig, or accept |
| QA-A-08 | Observation | With no adapter, `blueroute status` reports `Health: Error`; the procedure says "degraded". The journal shows the expected failed startup reconciliation (`CapabilityUnavailable`). | Align the procedure wording, or the health level |
| QA-A-09 | Runbook defect | The runbook repeats the removal helper after `apt remove`, when the helper is already deleted (rc 127). The repeat used `blueroute-daemon` extracted from the same `.deb` (sha256 above) against the unchanged state. | Fix the runbook order |
| QA-A-10 | Open question | `ensure_address` (`networkmanager/addresses.rs`) creates the owned profile before activating it and does not delete it when activation fails. If a caller does not record the membership, removal later treats that profile as foreign and never removes it (seen with the fixture in QA-A-05). The fixture records membership only after success. Whether a production join path can leave such a profile could not be settled here; join is gated off. | Review the join path's failure handling |

Not covered: other distributions, other NetworkManager or systemd versions, real Bluetooth
adapters and PAN links (Part B), and the "with `bluetoothd` running" variants (QA-A-07).

Raw evidence (`qa-evidence-*.tar.gz` per case, plus every command's output) was kept on the test
workstation, not committed; it contains only the VM's synthetic host name and virtual MAC
addresses.

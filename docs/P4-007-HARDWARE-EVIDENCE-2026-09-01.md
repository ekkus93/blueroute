# P4-007 Hardware NetworkManager Evidence — 2026-09-01

This record captures physical-host acceptance of the P4-007 NetworkManager backend.

## Environment

- Host: `debiancb1`.
- OS: Debian GNU/Linux 13 (trixie).
- Kernel: `6.12.86+deb13-amd64`.
- BlueZ: `5.82`.
- NetworkManager: `1.52.1`.
- Accepted BlueRoute revision: `269023fcd72c6ee0db2a23eb9bcc1dc05d300304` on branch `P4-007_networkmanager_backend`.
- Probe: `crates/blueroute-linux/examples/networkmanager_probe.rs`.
- Test bridge: `br-blue-nm`.
- Test address: `10.254.89.1/30`.

## Authorization behavior

The probe was first run as the normal login user. Read-only NetworkManager D-Bus access succeeded and the backend enumerated 15 connection profiles and 4 devices, but NetworkManager rejected the mutating `AddConnection2` call with `org.freedesktop.NetworkManager.Settings.PermissionDenied: Insufficient privileges`.

The already-built probe binary was then run with `sudo`; `cargo` itself was not run as root. This confirmed that the backend reports the local NetworkManager/Polkit authorization boundary instead of silently falling back to shell commands or `nmcli` parsing.

## Hardware-discovered compatibility defect and remediation

The first privileged acceptance attempt created and activated the BlueRoute-owned bridge and observed its connection/device events, but `ensure_address()` failed because NetworkManager rejected `ipv4.method=manual` as containing no address or route.

The live failure exposed a serialization compatibility defect: `GetSettings()` can return NetworkManager's deprecated `addresses` property, and NetworkManager ignores modern `address-data` when the legacy property is present in the same update. The backend was corrected to remove `addresses` before writing `address-data`, and a regression test was added. The accepted run below was performed after that fix and after the full locked CI suite passed.

## Accepted probe result

The final privileged probe reported:

```text
NetworkManager version: 1.52.1
baseline: connections=15 devices=4 foreign-profiles=15
bridge ready: interface=br-blue-nm profile=/org/freedesktop/NetworkManager/Settings/18 owner=47474747474747474747474747474747 repeated-ensure=same-profile
observed NetworkManager connection event for br-blue-nm
observed NetworkManager device event for br-blue-nm
address ready: interface=br-blue-nm address=10.254.89.1/30 repeated-ensure=present
foreign NetworkManager profiles preserved after setup
cross-owner bridge takeover rejected: refusing to create a BlueRoute profile for an interface claimed by another NetworkManager profile
cross-owner rejection left no leaked profile
wrong-owner cleanup was a safe no-op
holding configured bridge for 90s; verify the live kernel address in another terminal with: ip -4 addr show br-blue-nm
address removed; repeated remove succeeded
observed NetworkManager connection removal for br-blue-nm
observed NetworkManager device removal for br-blue-nm
bridge/profile removed; repeated cleanup succeeded
foreign NetworkManager profiles preserved after cleanup
P4-007 NetworkManager probe PASS
```

The live kernel inspection during the hold window independently showed that NetworkManager had created the bridge and applied the requested IPv4 address:

```text
7: br-blue-nm: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 ... state DOWN ...
    link/ether ce:99:df:04:84:28 brd ff:ff:ff:ff:ff:ff
    inet 10.254.89.1/30 brd 10.254.89.3 scope global noprefixroute br-blue-nm
       valid_lft forever preferred_lft forever
```

`NO-CARRIER` / `state DOWN` was expected for this isolated P4-007 bridge because no PAN/BNEP member interface was attached during the NetworkManager-only test.

## Ownership and cleanup evidence

The acceptance run proved all ownership-sensitive behavior required by P4-007:

- repeated `ensure_bridge()` reused the same BlueRoute-owned NetworkManager profile;
- repeated `ensure_address()` converged on `10.254.89.1/30`;
- all 15 pre-existing foreign NetworkManager profiles remained present and foreign after setup;
- a second BlueRoute `NetworkId` was refused ownership of the same bridge;
- the rejected cross-owner attempt left no profile behind;
- wrong-owner cleanup was a no-op;
- repeated address removal succeeded;
- repeated bridge/profile cleanup succeeded;
- the exact `br-blue-nm` connection and device removal events were observed;
- all 15 baseline foreign profiles remained preserved after cleanup.

No production operation used or parsed `nmcli` output. NetworkManager state and mutation were performed through the Rust system-D-Bus backend.

## Acceptance status

P4-007 hardware acceptance is satisfied on this Debian 13 / NetworkManager 1.52.1 host. The Rust backend enumerated live NetworkManager state, observed changes, created and activated a BlueRoute-owned bridge, applied and removed BlueRoute-owned addressing, rejected cross-owner takeover without leaking state, and removed only BlueRoute-owned state while preserving foreign profiles.

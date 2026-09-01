# P4-006 hardware acceptance evidence — 2026-08-31

## Scope

This record captures the real two-node hardware acceptance for **P4-006 — Implement NAP lifecycle adapter**.

The acceptance objective was to replace the manual NAP side of the earlier PAN experiments with BlueRoute's Rust `org.bluez.NetworkServer1` implementation and prove that it works with the existing Rust PANU path, including NAP-side client attach/detach observation and ordinary IP traffic over BNEP.

## Test topology

- NAP/server Linux node: `debiancb1`.
- PANU/client Linux node: `arisu`.
- Both Rust backends selected `/org/bluez/hci0` as their powered Bluetooth adapter.
- NAP peer identity observed by the PANU probe: `debiancb1`, paired and trusted.
- NAP bridge: `br-blue-test`.
- Dedicated test subnet: `10.254.88.0/30`.
  - `debiancb1` / `br-blue-test`: `10.254.88.1/30`.
  - `arisu` / PANU interface: `10.254.88.2/30`.
- Final same-revision acceptance run: both nodes on `master` commit `c3721ccae8ae2c4d98a0b6192994a5d83edd9ecd` (`feat: implement BlueZ NAP lifecycle`).

The broader per-host distribution/kernel/BlueZ/NetworkManager inventory remains tracked by P1-001. This focused record captures the P4-006 Rust NAP/PANU behavior, as the preceding P4-003 through P4-005 hardware evidence records do.

## Bridge setup and documentation defect found during acceptance

The first documented bridge name, `br-blueroute-test`, failed at `ip link add` because it exceeds Linux's 15-character interface-name limit. The failure occurred before the Rust NAP path was invoked and therefore was not a NAP implementation failure.

The acceptance run switched to the valid name `br-blue-test`:

```text
3: br-blue-test: <NO-CARRIER,BROADCAST,MULTICAST,UP> ...
    inet 10.254.88.1/30 scope global br-blue-test
```

`docs/P4-006-NAP.md` is corrected alongside this evidence so its example uses a valid Linux interface name.

## Rust NAP registration and client attachment

On `debiancb1`, current `master` ran:

```text
cargo run -p blueroute-linux --example bluez_nap_probe --locked -- br-blue-test 600
adapter: /org/bluez/hci0
NAP registered: bridge=br-blue-test hold=600s
BlueZ now accepts PANU clients into the supplied bridge. Bridge creation and IP addressing are intentionally outside P4-006.
NAP client attached: interface=enxf4d10870b786
```

This proves that the Rust path successfully registered the local NAP through `NetworkServer1.Register("nap", "br-blue-test")` and that the NAP event subscription observed the accepted Bluetooth network interface using its post-udev Linux name.

## Rust PANU and data-plane proof

For the first data-plane run, `arisu` began on commit `acee85b6850eb107a0f64af570d40b64eb80479a`, the already-accepted P4-005 PANU revision. The `bluez_panu_probe` source is unchanged between that revision and `c3721cc`, and `arisu` was subsequently fast-forwarded to `c3721cc` for the final same-revision attach/detach run described below.

The PANU probe established the link as:

```text
adapter: /org/bluez/hci0
discovering for 10 seconds...
target: /org/bluez/hci0/dev_F4_D1_08_70_B7_86 name=debiancb1 paired=true trusted=true
PANU connected: interface=enxe8fb1c25e4c2 hold=600s
```

The client interface was assigned the other address in the dedicated `/30`:

```text
12: enxe8fb1c25e4c2: <BROADCAST,MULTICAST,UP,LOWER_UP> ...
    inet 10.254.88.2/30 scope global enxe8fb1c25e4c2

10.254.88.1 dev enxe8fb1c25e4c2 src 10.254.88.2
```

The decisive data-plane test was explicitly bound to the PANU interface:

```text
ping -I enxe8fb1c25e4c2 -c 10 10.254.88.1

10 packets transmitted, 10 received, 0% packet loss, time 9013ms
rtt min/avg/max/mdev = 18.308/27.635/34.234/5.359 ms
```

This is direct evidence that IPv4/ICMP traffic traversed the Bluetooth BNEP PAN from the Rust PANU client into the bridge supplied to the Rust NAP server.

## Idempotent NAP teardown

At the end of the first NAP hold window, `debiancb1` reported:

```text
NAP hold window elapsed
NAP stopped; repeated stop succeeded
```

The probe deliberately invokes `stop_nap` twice. This therefore exercises and proves the real-hardware desired-state idempotence of NAP teardown.

The corresponding PANU probe later completed its own bounded hold and repeated disconnect successfully:

```text
PANU hold window elapsed
PANU disconnected; repeated disconnect succeeded
```

## Same-revision attach/detach observation

Before the final short run, `arisu` fast-forwarded cleanly from `acee85b` to current `master` `c3721cc`; both test nodes were therefore on the exact P4-006 merge revision.

`debiancb1` ran a 120-second NAP probe and `arisu` ran a 20-second PANU probe. The PANU connected and then deliberately disconnected before the NAP observation window ended:

```text
PANU connected: interface=enxe8fb1c25e4c2 hold=20s
PANU hold window elapsed
PANU disconnected; repeated disconnect succeeded
```

During that same run, the Rust NAP event stream on `debiancb1` emitted both sides of the client lifecycle:

```text
NAP registered: bridge=br-blue-test hold=120s
NAP client attached: interface=enxf4d10870b786
NAP client detached: interface=enxf4d10870b786
```

This confirms that authoritative bridge-membership reconciliation observes both accepted-client attachment and later detachment on real BlueZ/BNEP hardware.

## Acceptance conclusion

P4-006 acceptance requirement:

> Rust path replaces manual P1 setup for a supported NAP/PANU pair.

**Result: PASS.**

The tested Rust path successfully:

- registered a NAP through BlueZ `NetworkServer1` on an existing Linux bridge;
- accepted the Rust PANU client;
- observed the server-side post-udev Bluetooth interface attach;
- carried ordinary IPv4/ICMP traffic across BNEP with 0% packet loss in the acceptance sample;
- observed the client detach in a final same-revision run; and
- stopped the owned NAP twice successfully, proving idempotent cleanup on the physical stack.

P4-006 can therefore be marked complete. Bridge creation, production address management, DHCP/DNS, and NetworkManager-owned network lifecycle remain P4-007 and later work.

# P4-005 hardware acceptance evidence — 2026-08-30

## Scope

This records the real-hardware acceptance run for **P4-005 — Implement PANU connection adapter**.

## Test nodes

- PANU/client: `arisu`
- NAP/server: `debiancb1`
- Remote NAP Bluetooth address: `F4:D1:08:70:B7:86`

`debiancb1` exposed a NetworkManager Bluetooth NAP profile (`bluetooth.type=nap`, `ipv4.method=shared`) on `btnap0`. BlueZ advertised the standard NAP UUID `00001116-0000-1000-8000-00805f9b34fb`, and `arisu` resolved `org.bluez.Network1` for the peer.

## PANU establishment

On `arisu`, the Rust hardware probe was run from merged `master`:

```text
cargo run -p blueroute-linux --example bluez_panu_probe --locked -- debiancb1 600
adapter: /org/bluez/hci0
discovering for 10 seconds...
target: /org/bluez/hci0/dev_F4_D1_08_70_B7_86 name=debiancb1 paired=true trusted=true
PANU connected: interface=enxe8fb1c25e4c2 hold=600s
The BNEP link is up. IP addressing is intentionally outside P4-005; configure test addresses separately if validating the data plane.
```

The interface name is significant. BlueZ initially created/reported `bnep0`, but systemd-udevd renamed the live Linux netdev to the predictable name `enxe8fb1c25e4c2`. Hardware testing exposed this mismatch, and PR #17 / merge commit `acee85b6850eb107a0f64af570d40b64eb80479a` added post-udev interface-name reconciliation. The successful probe above demonstrates the fix on the affected host.

## Dedicated data-plane test

To avoid accidentally testing against another local `10.42.0.0/24` bridge/profile, the acceptance run used a dedicated temporary `/30`:

- `debiancb1` / `btnap0`: `10.254.87.1/30`
- `arisu` / `enxe8fb1c25e4c2`: `10.254.87.2/30`

`arisu` showed the PANU interface up with lower-layer carrier:

```text
16: enxe8fb1c25e4c2: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 ...
    inet 10.254.87.2/30 scope global enxe8fb1c25e4c2
```

The data-plane probe was bound explicitly to the PANU interface:

```text
ping -I enxe8fb1c25e4c2 -c 10 10.254.87.1

10 packets transmitted, 10 received, 0% packet loss, time 9013ms
rtt min/avg/max/mdev = 18.934/27.307/37.138/6.924 ms
```

This is unambiguous evidence that IPv4 packets traversed the Bluetooth BNEP PAN link between the two Linux test nodes.

## Acceptance conclusion

P4-005 acceptance requirement:

> Hardware integration path creates a working PANU data plane on supported test hardware.

**Result: PASS.**

The Rust PANU path successfully discovered the paired/trusted Linux NAP peer, established `org.bluez.Network1.Connect("nap")`, resolved the live post-udev Linux interface name, and carried bidirectional IPv4/ICMP traffic over BNEP with 0% packet loss in the acceptance sample.

# P6-001 hardware acceptance evidence — 2026-09-01

## Scope

This record captures the physical two-node acceptance for **P6-001 — Implement create-network operation**. The acceptance objective was to prove the complete production path from the daemon API through durable membership, NetworkManager-owned bridge/address creation, BlueZ NAP registration, and a real PANU attachment into the exact daemon-created bridge.

## Test topology

- NAP/server: `debiancb1`.
- PANU/client: `arisu`.
- Candidate branch: `P6-001_create_network`.
- Hardware-tested candidate before evidence-only closeout commits: `c16c5273bf56b5cb99d047f1bf724b4d1820dc36`.
- Server BlueZ adapter: `/org/bluez/hci0`, Linux driver `btusb`.
- Server Bluetooth address observed by the PANU peer path: `F4:D1:08:70:B7:86`.
- Client Bluetooth controller identity from prior accepted PANU evidence: `E8:FB:1C:25:E4:C2` (`arisu`).
- Server baseline: Debian GNU/Linux 13, kernel `6.12.86+deb13-amd64`, BlueZ `5.82`, NetworkManager `1.52.1`, systemd `257`.
- The PANU client is the same `arisu` hardware path already accepted in P4-005/P4-006; this P6-001 record uses it only to prove the daemon-created NAP attaches a real BNEP port to the exact generated bridge.

## Clean pre-create baseline

Before `CreateNetwork`, `debiancb1` had no durable BlueRoute membership and no `brb-*` interface/profile:

```text
--- pre-create baseline ---
No membership state yet

No BlueRoute bridge yet

No BlueRoute bridge profile yet
```

This rules out accidentally accepting pre-existing P6 runtime state.

## Production CreateNetwork operation

The production system daemon was running the P6-001 candidate. The hardware create was issued through the versioned system-D-Bus `Request` method with the typed JSON command:

```json
{"type":"create_network","data":{"name":"P6 Hardware Test"}}
```

The request was issued as root only for this hardware-orchestration acceptance because the active shell was a remote SSH/TTY session and the installed PolicyKit action intentionally has `allow_any=no`. P5-007 separately proved that normal unprivileged callers are denied safely and that the daemon does not bypass PolicyKit. No authorization policy was weakened for P6-001.

Observed result:

```text
s "{\"type\":\"ack\"}"
create exit=0
```

The subsequent read-only `Status` response reported a non-null current network:

```text
current_network = 26ed3f29d622ae9c5c68635f4d548bbe
```

## Durable membership

The daemon persisted stable membership in `/var/lib/blueroute/memberships-v1`:

```text
BLUEROUTE_MEMBERSHIP_V2
network 26ed3f29d622ae9c5c68635f4d548bbe        member  50362048617264776172652054657374
```

The encoded display name corresponds to `P6 Hardware Test`. The persisted state is the stable `member` state; no transient `Joining` state was recorded.

## NetworkManager bridge and deterministic local subnet

The generated network ID deterministically produced bridge `brb-26ed3f29` and host address `10.201.41.1/24`.

Kernel state:

```text
brb-26ed3f29     DOWN           7e:56:20:8e:ec:5d <NO-CARRIER,BROADCAST,MULTICAST,UP>

3: brb-26ed3f29: <NO-CARRIER,BROADCAST,MULTICAST,UP> ...
    inet 10.201.41.1/24 brd 10.201.41.255 scope global noprefixroute brb-26ed3f29
```

NetworkManager state showed the BlueRoute-owned bridge profile active on that exact interface:

```text
blueroute-bridge-26ed3f29  db646231-78b4-535d-8796-902d6733e2d9  bridge  brb-26ed3f29

--- NetworkManager active connections ---
blueroute-bridge-26ed3f29  bridge  brb-26ed3f29
```

The bridge being `NO-CARRIER` before a PAN client attached was expected.

## Real PANU attachment into the exact daemon-created bridge

On `arisu`, the existing Rust PANU probe connected to the paired/trusted `debiancb1` peer while the daemon-created NAP remained active:

```text
adapter: /org/bluez/hci0
discovering for 10 seconds...
target: /org/bluez/hci0/dev_F4_D1_08_70_B7_86 name=debiancb1 paired=true trusted=true
PANU connected: interface=enxe8fb1c25e4c2 hold=300s
The BNEP link is up. IP addressing is intentionally outside P4-005; configure test addresses separately if validating the data plane.
```

During that same live 300-second PANU hold, `debiancb1` showed the corresponding server-side Bluetooth/BNEP interface `enxf4d10870b786` with `LOWER_UP` and, critically, with `master brb-26ed3f29`:

```text
brb-26ed3f29      DOWN     7e:56:20:8e:ec:5d <NO-CARRIER,BROADCAST,MULTICAST,UP>
enxf4d10870b786   UNKNOWN  f4:d1:08:70:b7:86 <BROADCAST,MULTICAST,UP,LOWER_UP>

5: enxf4d10870b786: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 master brb-26ed3f29 state listening priority 32 cost 100
```

The kernel bridge membership directory independently confirmed the same attachment:

```text
/sys/class/net/brb-26ed3f29/brif/enxf4d10870b786
  -> .../bluetooth/hci0/.../net/enxf4d10870b786/brport
```

and the explicit master scan returned:

```text
enxf4d10870b786
```

This is direct physical proof that the NAP registered by the production `CreateNetwork` path accepted a real PANU and attached the resulting Bluetooth netdev to the exact NetworkManager bridge created for network `26ed3f29d622ae9c5c68635f4d548bbe`.

## Capability and failure-policy coverage

Deterministic CI already covers the non-hardware acceptance branch: `CreateNetwork` fails closed when NAP capability is false or unknown rather than dispatching based on a computer/model name. It also covers setup ordering, rollback, duplicate-create rejection, durable commit ordering, and the D-Bus/PolicyKit dispatch boundary.

## Acceptance conclusion

P6-001 acceptance requirement:

> `CreateNetwork` yields stable daemon state on a NAP-capable Linux node.
>
> Unsupported NAP capability produces a clear error rather than a model-name special case.

**Result: PASS.**

The physical acceptance demonstrates the complete product path:

**CreateNetwork → durable member state → NetworkManager bridge/address → real BlueZ NAP → real PANU attachment into that exact bridge.**

No manual bridge, address, or NAP creation was used after invoking `CreateNetwork`. P6-001 can therefore be marked complete. Automatic client address allocation, discovery/advertisement, join approval, join orchestration, ordinary application traffic, leave, restart reconciliation, and reconnect remain later P6 tasks.
# P6-002 hardware acceptance evidence — 2026-09-02

## Scope

This record captures the physical two-node acceptance for **P6-002 — Implement discoverable BlueRoute network identity**.

The acceptance objective was to prove that a second compatible Linux node can discover a hosted BlueRoute network's exact logical `NetworkId` without being given a Bluetooth MAC address.

## Test topology

- Hosted BlueRoute network / NAP: `debiancb1`.
- Discovery client: `arisu`.
- Both nodes use the existing BlueRoute hardware path through BlueZ adapter `/org/bluez/hci0`.
- `debiancb1` controller path uses Linux driver `btusb`; its P6-001 baseline immediately preceding this test was Debian GNU/Linux 13, kernel `6.12.86+deb13-amd64`, BlueZ `5.82`, NetworkManager `1.52.1`, and systemd `257`.
- `arisu` is the same physical discovery/PANU client used in the accepted P4-003 through P4-006 and P6-001 hardware runs; its previously recorded controller identity is `E8:FB:1C:25:E4:C2`.
- Candidate branch: `P6-002_discoverable_network_identity`.
- Hardware-tested candidate head before evidence-only closeout commits: `1aea3a1310114da5a7490ce6fe024706eb0867ff`.
- PR: #37.
- CI run `33609623352` passed the full pinned Rust gate before hardware acceptance.

The focused P6-002 run did not re-collect a complete distribution/kernel/BlueZ/NetworkManager version inventory from `arisu`; the broader per-host inventory remains owned by P1-001. This record does not infer or fabricate unobserved client version values.

## Fresh hosted network

The P6-002 daemon was installed on `debiancb1`. The previous P6-001 membership record was removed while preserving the durable node identity, and the system service was restarted.

A fresh production `CreateNetwork` request was issued through the versioned system-D-Bus API:

```bash
sudo busctl --system call \
  org.blueroute.Service1 \
  /org/blueroute/Service1 \
  org.blueroute.Service1 \
  Request \
  s \
  '{"type":"create_network","data":{"name":"P6 Discovery Test"}}'
```

The daemon returned:

```text
s "{\"type\":\"ack\"}"
```

The durable membership record then contained:

```text
BLUEROUTE_MEMBERSHIP_V2
network 4bc4e5b829838b47c985ec66881306fa        member  503620446973636f766572792054657374
```

The hosted network identity for this acceptance run was therefore:

```text
4bc4e5b829838b47c985ec66881306fa
```

Because P6-002 makes LE advertisement registration required hosted-star runtime state, successful `CreateNetwork` means the daemon completed bridge/address setup, BlueZ NAP registration, and BlueRoute network advertisement registration before committing durable membership.

## Discovery client installation

`arisu` initially had no installed `blueroute.service`; this was an environment/setup issue rather than a discovery failure. The shipped daemon, systemd unit, D-Bus policy, and PolicyKit policy were installed unchanged from the P6-002 branch.

After `systemctl daemon-reload`, D-Bus reload, and service enable/start, the production service reported:

```text
Loaded: loaded (/lib/systemd/system/blueroute.service; enabled; vendor preset: enabled)
Active: active (running)
Main PID: 357973 (blueroute-daemon)
```

Any local `memberships-v1` record on `arisu` was removed before discovery so the result could not be satisfied by remembered durable membership.

## No-MAC discovery probe

The production client example was built and run on `arisu`:

```bash
cargo build --release \
  -p blueroute-client \
  --example network_discovery_probe \
  --locked

sudo ./target/release/examples/network_discovery_probe
```

The probe accepts no Bluetooth MAC-address argument. Internally it performs:

`StartDiscovery` → ten-second scan window → `ListNetworks` → `StopDiscovery`.

Observed output:

```text
discovering BlueRoute networks for 10 seconds...
network=4bc4e5b829838b47c985ec66881306fa name=BlueRoute 4bc4e5b8 member_count=0
```

The `NetworkId` observed on `arisu` exactly matches the fresh hosted network identity persisted by `debiancb1`:

```text
server: 4bc4e5b829838b47c985ec66881306fa
client: 4bc4e5b829838b47c985ec66881306fa
```

No Bluetooth MAC address was supplied to the client probe or used as the logical network identity.

## Security boundary

This evidence proves discoverability only. The advertised `NetworkId` remains unauthenticated discovery metadata and does not itself grant pairing, trust, membership, PANU attachment, IP configuration, or authenticated control-plane access. Those remain later tasks.

The hardware result therefore does not weaken the P6-002 security rule that Bluetooth name, alias, MAC address, RSSI, pairing status, and advertisement contents are not authorization evidence.

## Acceptance conclusion

P6-002 acceptance requirement:

> Second compatible Linux node discovers a candidate network without manual MAC entry.

**Result: PASS.**

The physical chain demonstrated was:

**CreateNetwork on `debiancb1` → BlueRoute LE advertisement → BlueZ discovery on `arisu` → exact logical `NetworkId` returned by `ListNetworks`, with no MAC supplied.**

Combined with the already-green deterministic CI coverage for advertisement parsing, malformed-record rejection, name-independence, remembered-network precedence, authorization, formatting, workspace check, Clippy, tests, D-Bus integrations, and packaging validation, P6-002 can be marked complete.

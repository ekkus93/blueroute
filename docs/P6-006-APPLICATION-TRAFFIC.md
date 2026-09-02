# P6-006 — Prove ordinary application traffic

## Scope

P6-006 proves that the IPv4 data plane produced by the BlueRoute Linux adapters carries ordinary application traffic. It does not add a BlueRoute application protocol and it does not weaken the P6-004 fail-closed join gate while authenticated P7 control bootstrap is still unavailable.

The acceptance matrix is:

- raw IPv4 ICMP with the system `ping` application;
- raw IPv4 SSH with the system OpenSSH client/server;
- TCP bulk transfer with integrity validation;
- UDP datagrams with payload and loss validation;
- a separate name-resolution observation that is never used to make the raw-IP tests pass.

The application layer must not import or link a BlueRoute library.

## Acceptance topology

P6-006 uses two acceptance-only setup probes. They compose the same production adapters that BlueRoute uses, but they do not call the still-blocked production `JoinNetwork` operation:

- `single_star_traffic_host` creates the NetworkManager-owned bridge/address and registers a BlueZ NAP;
- `single_star_traffic_client` establishes a BlueZ PANU and applies the deterministic first-client address with NetworkManager.

The host supports an `auto` selector for acceptance. In this mode it generates fresh `NetworkId` candidates until the P6-005 deterministic subnet derived from the candidate does not conflict with active local IPv4 state. The selected `network=` value printed by the host is then passed explicitly to the client. Supplying an explicit `NetworkId` never remaps it to another subnet; a conflict remains a fail-closed error.

Both probes check for a live IPv4 conflict before mutation and use owner-scoped NetworkManager cleanup. They do not configure DNS, `/etc/hosts`, forwarding, NAT, or any application-specific route.

The PANU interface is created asynchronously by BlueZ. The client therefore waits, for a bounded eight-second window, for NetworkManager to observe that kernel interface before asking NetworkManager to create/apply BlueRoute-owned address state. A timeout fails closed and disconnects the PANU; there is no shell-command or foreign-profile fallback.

## Build

On both Linux test nodes, from the exact P6-006 branch revision:

```bash
cargo build --release -p blueroute-linux \
  --example single_star_traffic_host \
  --example single_star_traffic_client \
  --locked
```

The Python socket helper requires only Python 3 and the standard library.

## Start the host/NAP

On the NAP node:

```bash
sudo ./target/release/examples/single_star_traffic_host auto 600
```

The host prints the selected network and addresses, for example:

```text
P6-006 auto-selected conflict-free network on attempt 1: network=<network-id> segment=<segment>/24
P6-006 host ready
network=<network-id>
segment=<segment>/24 host=<host-ip>/24 client=<client-ip>/24 bridge=<bridge>
```

Leave the host probe running during all traffic checks. Record the exact `network=`, host IP, client IP, and bridge values; all following commands must use those actual values.

An explicit network can still be tested with:

```bash
sudo ./target/release/examples/single_star_traffic_host <network-id> 600
```

If its deterministic subnet conflicts, the probe fails closed and prints the conflict diagnostic instead of choosing a different subnet.

## Start the client/PANU

On the PANU node, replace `<nap-bluetooth-name>` and `<network-id>` with the NAP Bluetooth display name and the exact `network=` value printed by the host:

```bash
sudo ./target/release/examples/single_star_traffic_client \
  <nap-bluetooth-name> \
  <network-id> \
  600
```

The client does not have a default network ID. This prevents an acceptance run from silently deriving a segment different from the one selected by the host.

The probe prints the actual PANU interface and the derived host/client addresses. Leave it running during all traffic checks.

## Raw-IP tests

Run these from the PANU/client node unless otherwise stated. Replace `<host-ip>` with the host address printed by the host/client probes.

### ICMP

```bash
ping -c 10 <host-ip>
```

Acceptance requires replies over the BlueRoute segment with no hidden hostname dependency.

### SSH

Use an existing account and normal SSH authentication on the NAP node:

```bash
ssh <nap-user>@<host-ip> 'printf "P6-006 SSH PASS\\n"'
```

Do not add a special BlueRoute SSH transport or proxy. Normal OpenSSH must operate directly on the BlueRoute IPv4 address.

### TCP bulk transfer

On the NAP node:

```bash
python3 scripts/p6_006_socket_traffic.py tcp-server --bind <host-ip>
```

On the PANU node:

```bash
python3 scripts/p6_006_socket_traffic.py tcp-client <host-ip> --bytes 16777216
```

The helper uses only Python's standard `socket` and `hashlib` modules. The client sends 16 MiB and requires the server to report the identical byte count and SHA-256 digest.

### UDP

On the NAP node:

```bash
python3 scripts/p6_006_socket_traffic.py udp-server \
  --bind <host-ip> --count 256 --payload 1024
```

On the PANU node:

```bash
python3 scripts/p6_006_socket_traffic.py udp-client \
  <host-ip> --count 256 --payload 1024
```

Each datagram carries a sequence number and deterministic payload. Acceptance requires all 256 datagrams to arrive with valid payloads. This is an application-layer loss/integrity check, not a BlueRoute protocol.

## Name-resolution observation is separate

Only after the raw-IP tests have passed, record ordinary resolver behavior from the PANU node, for example:

```bash
getent ahostsv4 <nap-hostname>
getent ahostsv4 <nap-hostname>.local
```

A name may resolve because the surrounding Linux environment already provides DNS, mDNS/Avahi, or another resolver source; it may also fail. P6-006 does not require BlueRoute-provided DNS. The result must be recorded separately and must not be used to reinterpret a raw-IP failure as success.

Do **not** add `/etc/hosts` entries or another name-resolution fallback solely to make this acceptance run pass.

## Cleanup and evidence

Let both setup probes exit normally so they exercise owner-scoped NetworkManager cleanup and idempotent BlueZ teardown. Record:

- exact Git commit on both nodes;
- node names and roles;
- selected `NetworkId`, segment, host address, and client address;
- printed bridge/PANU interface names;
- raw IPv4 ping result;
- SSH command result;
- TCP byte count and SHA-256;
- UDP sent/received/loss result;
- name-resolution result as a separate observation;
- final host/client cleanup results.

CI can validate the probes and helper, but only a physical two-node run can satisfy the P6-006 traffic acceptance criterion.

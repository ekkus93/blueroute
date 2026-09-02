# P6-003 hardware acceptance evidence — 2026-09-02

## Scope

This record captures physical acceptance for **P6-003 — Implement join approval/trust workflow**.

The acceptance objective was to prove the initial trust model on real nodes without collapsing Bluetooth transport trust into BlueRoute membership:

- use the real stable `NodeId` of the intended peer;
- explicitly approve that `NodeId` through the production daemon API;
- persist the peer as trusted but not yet joined;
- make repeated approval idempotent;
- explicitly revoke the approval;
- preserve the logical network while revoking the peer.

Bluetooth pairing/trust itself was already proven on these physical nodes by P4-004. The focused P6-003 run therefore did not manufacture a new identity binding from a MAC address or repeat pairing merely for ceremony; it exercised the new BlueRoute authorization layer on top of the already-proven transport-trust primitive.

## Test topology and baseline

- Hosted BlueRoute network / approval authority: `debiancb1`.
- Intended peer: `arisu`.
- Current network: `4bc4e5b829838b47c985ec66881306fa` (`P6 Discovery Test`), created during accepted P6-002 hardware validation.
- `arisu` stable BlueRoute `NodeId`: `4d5f049ad9b6ea51d782250478fa3ebf`, read directly from `/var/lib/blueroute/node-id`.
- `debiancb1` uses the existing BlueRoute hardware path through BlueZ adapter `/org/bluez/hci0` and Linux `btusb`.
- The immediately preceding P6 hardware baseline on `debiancb1` recorded Debian GNU/Linux 13, kernel `6.12.86+deb13-amd64`, BlueZ `5.82`, NetworkManager `1.52.1`, and systemd `257`.
- `arisu` is the same physical peer used in accepted P4-003 through P4-006, P6-001, and P6-002 runs; its previously recorded Bluetooth controller address is `E8:FB:1C:25:E4:C2`.
- Candidate branch: `P6-003_join_approval_trust`.
- Hardware-tested candidate head: `71b4456b546c3fbfbb8082e57960a87d8be892c3`.
- PR: #38.
- CI run `33666049233` passed formatting, workspace check, Clippy, all tests, daemon D-Bus integration, daemon authorization integration, client D-Bus integration, systemd validation, and D-Bus/PolicyKit packaging before hardware acceptance.

The focused P6-003 run did not re-collect a complete client distribution/kernel/BlueZ/NetworkManager inventory from `arisu`; the broader per-host inventory remains owned by P1-001. No unobserved client version values are inferred here.

## Real peer identity

On `arisu`, the persisted BlueRoute node identity was read directly:

```bash
sudo cat /var/lib/blueroute/node-id
```

Observed value:

```text
4d5f049ad9b6ea51d782250478fa3ebf
```

That exact `NodeId`, rather than a Bluetooth MAC address, display name, alias, or P6-002 advertisement field, was supplied to the BlueRoute approval operation.

## Candidate daemon and existing network

The P6-003 candidate daemon was built and installed on `debiancb1`. The production system service reported active.

Before approval, durable membership state contained only the existing P6-002 network:

```text
BLUEROUTE_MEMBERSHIP_V2
network 4bc4e5b829838b47c985ec66881306fa        member  503620446973636f766572792054657374
```

The network record was deliberately preserved; P6-003 tests trust mutation inside an existing logical network rather than creating a replacement network.

## Explicit approval

`debiancb1` invoked the versioned production D-Bus API with the real `arisu` `NodeId`:

```bash
sudo busctl --system call \
  org.blueroute.Service1 \
  /org/blueroute/Service1 \
  org.blueroute.Service1 \
  Request \
  s \
  "{\"type\":\"trust_peer\",\"data\":{\"node\":\"4d5f049ad9b6ea51d782250478fa3ebf\"}}"
```

The daemon returned:

```text
s "{\"type\":\"ack\"}"
```

Durable state then contained:

```text
BLUEROUTE_MEMBERSHIP_V2
network 4bc4e5b829838b47c985ec66881306fa        member  503620446973636f766572792054657374
peer    4bc4e5b829838b47c985ec66881306fa        4d5f049ad9b6ea51d782250478fa3ebf        0        1
```

For the peer record, the final fields are:

```text
member=0
trusted=1
```

This is the required separation: explicit approval is durable trust to attempt later join orchestration, not a false claim that P6-004 networking/control-plane join has already succeeded.

## Idempotent reapproval

After the first approval, the membership file SHA-256 was:

```text
82d00492ea6fed7eac3d015d135be03eaf47d03f8c608ac7b0166789d303486c  /var/lib/blueroute/memberships-v1
```

The same `TrustPeer` request was issued again. The daemon again acknowledged the request, and the file SHA-256 remained exactly:

```text
82d00492ea6fed7eac3d015d135be03eaf47d03f8c608ac7b0166789d303486c  /var/lib/blueroute/memberships-v1
```

The unchanged hash proves repeated approval did not rewrite or otherwise perturb already-correct durable state.

## Explicit revocation

The same real `NodeId` was revoked through the production API:

```bash
sudo busctl --system call \
  org.blueroute.Service1 \
  /org/blueroute/Service1 \
  org.blueroute.Service1 \
  Request \
  s \
  "{\"type\":\"forget_peer\",\"data\":{\"node\":\"4d5f049ad9b6ea51d782250478fa3ebf\"}}"
```

The daemon returned:

```text
s "{\"type\":\"ack\"}"
```

Durable state after revocation was:

```text
BLUEROUTE_MEMBERSHIP_V2
network 4bc4e5b829838b47c985ec66881306fa        member  503620446973636f766572792054657374
```

The peer record was gone while the network record remained intact.

## Pair-if-needed coverage and security boundary

P6-003 reuses the P4-004 BlueZ pairing/trust adapter, whose real two-node pairing/trust behavior is already recorded in `docs/P4-004-HARDWARE-EVIDENCE-2026-08-30.md`. The P6-003 implementation composes those idempotent primitives for a caller-selected BlueZ peer but deliberately does not infer a `NodeId` from BlueZ `Device1`, MAC address, name, alias, RSSI, or pairing state.

This focused acceptance therefore proves the new BlueRoute approval layer, while retaining the architecture's two independent facts:

1. Bluetooth pairing/trust is transport-level state.
2. BlueRoute `NodeId` approval is network-authorization state.

Neither layer alone marks a peer joined. P6-004 must still establish PANU/IP/control-session state and use the fail-closed approval guard before committing active membership.

## Acceptance conclusion

P6-003 acceptance requirement:

> Joining requires intended trust under initial security model.

**Result: PASS.**

The physical chain demonstrated was:

**real persisted `arisu` NodeId → PolicyKit-gated `TrustPeer` → durable `trusted=1, member=0` → idempotent repeated approval with unchanged file hash → `ForgetPeer` → approval removed while the network remains.**

Combined with the already-green CI coverage and the previously accepted P4-004 physical Bluetooth pairing/trust evidence, all P6-003 subtasks and acceptance requirements are satisfied.

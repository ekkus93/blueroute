# P6-005 hardware evidence — one-star IPv4 allocation

Date: 2026-09-02

## Scope

This record captures live Debian hardware acceptance for the P6-005 IPv4 allocation implementation on `debiancb1` using the production `NetworkManagerBackend` through `ipv4_allocation_probe`.

This evidence covers deterministic one-star address planning plus repeated apply/remove cleanup. It does not claim that P6-004 authenticated `JoinNetwork` is complete; the remaining authenticated control-session prerequisite is owned by P7.

## Code under test

Branch:

```text
P6-005_ipv4_allocation
```

Commit:

```text
df2da55cf3a1fac1a903407467973ad18801e60f
```

The checkout was verified with `git rev-parse HEAD` before building the probe.

## Build and execution

The probe was built and run on `debiancb1` with:

```bash
cargo build --release \
  -p blueroute-linux \
  --example ipv4_allocation_probe \
  --locked

sudo ./target/release/examples/ipv4_allocation_probe
```

The build completed successfully.

## Observed allocation

The probe selected test network identity:

```text
65656565656565656565656565656565
```

and deterministically derived:

```text
segment=10.201.101.0/24
host=10.201.101.1/24
first_client=10.201.101.2/24
```

This matches the P6-005 policy of one `/24` segment derived from `NetworkId`, with the NAP/host at `segment + 1` and the first PANU/client at `segment + 2`.

## Repeated NetworkManager apply/remove acceptance

Cycle 1:

```text
cycle=1 applied=10.201.101.1/24
cycle=1 cleanup=clean
```

Cycle 2:

```text
cycle=2 applied=10.201.101.1/24
cycle=2 cleanup=clean
```

Final result:

```text
P6-005 IPv4 allocation probe PASS
```

For each cycle the production backend created the BlueRoute-owned NetworkManager bridge/address state, removed the address/profile again, and then re-checked the segment for conflicts. The second cycle succeeded on the same planned segment, demonstrating that cleanup did not leave a stale address/profile that would block subsequent use.

## Acceptance conclusion

The live probe demonstrates the P6-005 hardware acceptance property that repeated allocation/apply/remove cycles do not accumulate conflicts on the tested Debian host.

P6-005 is not considered complete from this hardware evidence alone. Final closeout also requires the full repository CI matrix to pass on the formatting-corrected branch head.

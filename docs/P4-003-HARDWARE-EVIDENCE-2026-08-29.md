# P4-003 Hardware Discovery Evidence — 2026-08-29

This record captures the real-radio hardware validation of the P4-003 BlueZ discovery adapter.

## Environment

- BlueRoute built from current `master` with the pinned Rust toolchain.
- Probe: `crates/blueroute-linux/examples/bluez_discovery_probe.rs`.
- Linux Bluetooth adapter selected by the Rust backend: `/org/bluez/hci0`.
- Discovery window: 10 seconds.

## Observed result

The Rust BlueRoute adapter successfully:

- started BlueZ discovery through `org.bluez.Adapter1.StartDiscovery`;
- completed the bounded 10-second discovery window;
- enumerated nearby `org.bluez.Device1` objects through the BlueZ object manager;
- mapped device display names;
- mapped both paired and unpaired devices;
- mapped both trusted and untrusted devices;
- stopped discovery cleanly and returned control to the probe.

A follow-up acceptance run discovered 13 device objects, including the known nearby Linux test node `debiancb1` as `/org/bluez/hci0/dev_F4_D1_08_70_B7_86` with `paired=false` and `trusted=false`.

The operator explicitly identified `debiancb1` as the Linux test node used for this acceptance run.

The raw scans also contained nearby third-party device addresses and names, so unrelated identifiers are intentionally not copied into the repository.

## Acceptance status

P4-003 acceptance is satisfied. A known nearby compatible Linux test node (`debiancb1`) appeared through the Rust BlueRoute discovery adapter during a real 10-second BlueZ discovery run.

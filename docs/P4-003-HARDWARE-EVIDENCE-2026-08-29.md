# P4-003 Hardware Discovery Evidence — 2026-08-29

This record captures the first real-radio hardware validation of the P4-003 BlueZ discovery adapter.

## Environment

- BlueRoute built from current `master` with the pinned Rust toolchain.
- Probe: `crates/blueroute-linux/examples/bluez_discovery_probe.rs`.
- Linux Bluetooth adapter selected by the Rust backend: `/org/bluez/hci0`.
- Discovery window: 10 seconds.

## Observed result

The Rust BlueRoute adapter successfully:

- started BlueZ discovery through `org.bluez.Adapter1.StartDiscovery`;
- completed the bounded 10-second discovery window;
- enumerated 59 `org.bluez.Device1` objects through the BlueZ object manager;
- mapped device display names;
- mapped both paired and unpaired devices;
- mapped both trusted and untrusted devices;
- stopped discovery cleanly and returned control to the probe.

The raw scan output contained nearby third-party device addresses and names, so those identifiers are intentionally not copied into the repository.

## Acceptance status

This run proves that P4-003 works against a real Linux Bluetooth controller and nearby radios rather than only against unit tests/CI.

The P4-003 acceptance criterion specifically requires a **nearby compatible Linux test node** to appear through the Rust adapter. The supplied scan shows many nearby devices, but the output alone does not establish which, if any, is a known Linux test node. Therefore P4-003 remains `[-]` until one discovered entry is positively identified as a Linux test node (or a second Linux test machine is made discoverable and observed by the probe).

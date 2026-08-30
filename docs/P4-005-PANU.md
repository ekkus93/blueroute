# P4-005 — BlueZ PANU connection adapter

P4-005 implements the client side of Bluetooth PAN/BNEP through BlueZ `org.bluez.Network1` on the selected remote `Device1` object.

## Selected API

BlueRoute uses BlueZ directly for PANU link lifecycle:

- service: `org.bluez`
- interface: `org.bluez.Network1`
- object: the selected peer `/org/bluez/{adapter}/dev_*` object
- connect: `Connect("nap")`
- disconnect: `Disconnect()`

`Connect("nap")` means the local machine joins the remote peer's NAP service as a PANU. BlueZ returns the created network-interface name, normally a `bnep*` interface. That interface is mapped into the backend-neutral `NetworkInterfaceHandle` and returned in a `PanAttachment` with local role `PanRole::Panu`.

This task deliberately does not assign IP addresses, routes, DNS, or forwarding state. Those responsibilities remain behind `IpNetworkBackend` and the later NetworkManager/route tasks. Keeping the BNEP-link lifecycle separate prevents the core topology model from depending on NetworkManager-specific objects.

## Connection behavior

`BluezBackend` implements `PanBackend` for PANU operations.

A successful connection returns:

- local role `Panu`;
- the BlueZ-created interface name;
- the selected peer handle.

If BlueZ reports `AlreadyConnected`, BlueRoute reconciles the authoritative `Network1.Connected` and `Network1.Interface` properties and returns the existing attachment rather than treating an already-satisfied connection as a fatal error.

Common BlueZ errors are mapped into typed `CoreError` categories. Missing/unsupported PAN profile APIs map to `CapabilityUnavailable`, authorization failures map to `AuthenticationFailed`, and connection failures map to `PanFailure`.

## Link-loss observation

`PanBackend::subscribe_panu_events` creates a bounded D-Bus signal stream. It listens for relevant `PropertiesChanged` notifications from both `org.bluez.Network1` and `org.bluez.Device1`, then re-reads the authoritative `Network1.Connected` state before emitting `PanuEvent::Lost`.

Using `Device1.Connected` as a secondary trigger is intentional. Some BlueZ teardown paths have historically been inconsistent about emitting a `Network1` property-change signal for local disconnects. The subscription also treats `ObjectManager.InterfacesRemoved` for the tracked `Network1` or `Device1` object as a state-refresh trigger. P4-005 therefore does not rely on one exact signal sequence, and every trigger is reconciled against authoritative `Network1` state before a loss event is emitted.

The subscription installs its bounded signal match before taking the initial link-state snapshot so a disconnect cannot fall into a check-then-subscribe race window. It retains only one tracked attachment and does not accumulate link history.

## Idempotent disconnect

`disconnect_panu` treats an already-absent peer/network connection as success when BlueZ reports an already-disconnected or disappeared object. It also checks authoritative state before issuing `Disconnect()` and re-checks state after an ambiguous method failure, so a teardown that already reached the requested disconnected state is still treated as success. The hardware probe calls disconnect twice so real acceptance can verify the desired-state operation is idempotent.

## Hardware probe

Build/run on the PANU machine:

```bash
cargo run -p blueroute-linux --example bluez_panu_probe --locked -- <peer-name> [hold-seconds]
```

The probe:

1. selects the first powered BlueZ adapter;
2. discovers for 10 seconds;
3. finds the named peer;
4. calls the Rust `PanBackend::connect_panu` path;
5. prints the resulting BNEP interface;
6. keeps the connection alive for a bounded hold window while watching for link loss;
7. disconnects twice to exercise idempotent teardown.

For P4-005 hardware acceptance, the remote Linux node must already provide a NAP service. During the hold window, test IP addresses may be configured externally on the two ends of the BNEP data plane until BlueRoute's IP/network backend is implemented. Acceptance must prove ordinary traffic traverses the Bluetooth PAN interface and must record the test systems and software versions; CI alone cannot satisfy that criterion.

## P4-006 boundary

NAP registration/acceptance is intentionally left to P4-006. The `BluezBackend` `start_nap` and `stop_nap` methods continue to return a typed capability-unavailable error until that task is implemented.

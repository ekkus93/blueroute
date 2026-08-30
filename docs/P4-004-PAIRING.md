# P4-004 BlueZ Pairing and Trust

BlueRoute performs pairing directly through `org.bluez.Device1.Pair` and changes BlueZ trust through the `org.bluez.Device1.Trusted` property. Production code does not invoke or parse `bluetoothctl`.

BlueZ pairing-agent management is performed through `org.bluez.AgentManager1` at `/org/bluez`; `/` remains reserved for BlueZ's `org.freedesktop.DBus.ObjectManager`.

## Application agent

Before an outgoing pairing operation, the Linux backend serves `org.bluez.Agent1` at `/org/blueroute/PairingAgent` and registers it with `org.bluez.AgentManager1` using the `NoInputNoOutput` capability. It is an application agent, not a requested system-wide default agent.

The agent authorizes just-works authorization and service callbacks only for the exact peer whose `BluetoothBackend::pair` operation is active. Unrelated callbacks are rejected. PIN-code input, passkey input, and numeric passkey confirmation requests are rejected because a `NoInputNoOutput` BlueRoute process cannot honestly satisfy them.

Only one outgoing pairing operation may be active through a backend instance at a time. Authorization is automatically revoked when that operation finishes or fails.

## Timeout and errors

BlueRoute bounds an outgoing pairing call to 60 seconds. On timeout it attempts `Device1.CancelPairing` before returning `ErrorKind::PairingFailed`.

BlueZ authentication rejection/cancellation/failure are translated into `ErrorKind::AuthenticationFailed`; connection and timeout failures use `ErrorKind::PairingFailed`. Invalid or stale requests retain typed state/input errors and low-level D-Bus context remains diagnostic-only.

## Trust policy

Pairing and BlueZ trust are deliberately separate. `pair()` does not silently mark a peer trusted. `set_trusted(peer, true)` requires the peer to already be paired; untrusting is always allowed. Neither operation changes BlueRoute network membership.

## Hardware acceptance probes

For a fully Rust-controlled two-node test, run the bounded incoming pairing window on the receiving Linux node first:

```bash
cargo run -p blueroute-linux --example bluez_pair_accept --locked
```

The acceptor temporarily registers BlueRoute as the BlueZ default `NoInputNoOutput` agent and enables `Pairable` and `Discoverable` on the selected adapter for 120 seconds. It authorizes only Device1 objects under that adapter, restores the adapter's previous pairable/discoverable values when the window closes, clears authorization before cleanup, and unregisters the BlueRoute agent so it does not remain the system-wide default.

Then, while that window is open, run the initiator from the other Linux node. The probe discovers a peer by exact BlueZ object path or exact display name, invokes the Rust pairing adapter, explicitly sets trust, and verifies the refreshed `Device1` state:

```bash
cargo run -p blueroute-linux --example bluez_pair_probe --locked -- debiancb1
```

`RequestDefaultAgent` may require additional system policy authorization on some Linux distributions. BlueRoute reports that as a typed capability/authentication failure rather than falling back to a graphical agent or `bluetoothctl`.

The P4-004 task remains in progress until two Linux test nodes complete this Rust-controlled acceptor/initiator flow and the evidence is recorded.

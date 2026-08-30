# P4-004 BlueZ Pairing and Trust

BlueRoute performs pairing directly through `org.bluez.Device1.Pair` and changes BlueZ trust through the `org.bluez.Device1.Trusted` property. Production code does not invoke or parse `bluetoothctl`.

## Application agent

Before an outgoing pairing operation, the Linux backend serves `org.bluez.Agent1` at `/org/blueroute/PairingAgent` and registers it with `org.bluez.AgentManager1` using the `NoInputNoOutput` capability. It is an application agent, not a requested system-wide default agent.

The agent authorizes just-works authorization and service callbacks only for the exact peer whose `BluetoothBackend::pair` operation is active. Unrelated callbacks are rejected. PIN-code input, passkey input, and numeric passkey confirmation requests are rejected because a `NoInputNoOutput` BlueRoute process cannot honestly satisfy them.

Only one outgoing pairing operation may be active through a backend instance at a time. Authorization is automatically revoked when that operation finishes or fails.

## Timeout and errors

BlueRoute bounds an outgoing pairing call to 60 seconds. On timeout it attempts `Device1.CancelPairing` before returning `ErrorKind::PairingFailed`.

BlueZ authentication rejection/cancellation/failure are translated into `ErrorKind::AuthenticationFailed`; connection and timeout failures use `ErrorKind::PairingFailed`. Invalid or stale requests retain typed state/input errors and low-level D-Bus context remains diagnostic-only.

## Trust policy

Pairing and BlueZ trust are deliberately separate. `pair()` does not silently mark a peer trusted. `set_trusted(peer, true)` requires the peer to already be paired; untrusting is always allowed. Neither operation changes BlueRoute network membership.

## Hardware acceptance probe

The probe below discovers a peer by exact BlueZ object path or exact display name, invokes the Rust pairing adapter, sets trust, and verifies the refreshed `Device1` state:

```bash
cargo run -p blueroute-linux --example bluez_pair_probe --locked -- debiancb1
```

The remote test node must be powered, discoverable/pairable as appropriate, and have an authentication agent capable of completing its side of the pairing exchange. The P4-004 task remains in progress until two Linux test nodes complete this Rust-initiated flow and the evidence is recorded.

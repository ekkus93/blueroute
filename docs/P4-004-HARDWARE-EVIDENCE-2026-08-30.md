# P4-004 Hardware Pairing Evidence — 2026-08-30

This record captures the real-radio two-node acceptance of the P4-004 BlueZ pairing/trust adapter.

## Test topology

- Receiving Linux test node: `debiancb1`.
- Initiating Linux test node: `arisu`.
- BlueRoute revision under test: `master` after `ab6b577f773e650a930d07064adcd8556cb55248` (`fix: use BlueZ AgentManager object path`).
- Both Rust probes selected `/org/bluez/hci0` as the powered Bluetooth adapter.
- Receiver probe: `crates/blueroute-linux/examples/bluez_pair_accept.rs`.
- Initiator probe: `crates/blueroute-linux/examples/bluez_pair_probe.rs`.

The broader platform/version inventory remains tracked by P1-001; this focused record captures the P4-004 pairing-path evidence and intentionally omits unrelated nearby Bluetooth identifiers.

## Receiver-side evidence

On `debiancb1`, the Rust acceptor was started with:

```bash
cargo run -p blueroute-linux --example bluez_pair_accept --locked
```

It successfully registered the BlueRoute Agent1 implementation through BlueZ AgentManager1, selected `/org/bluez/hci0`, and opened the bounded 120-second Rust-controlled incoming pairing window:

```text
adapter: /org/bluez/hci0
Rust-controlled incoming pairing window open for 120 seconds...
Run bluez_pair_probe from the other Linux test node now.
```

This validates the receiver-side default-agent registration and temporary incoming-pairing mode on real BlueZ hardware. An earlier hardware run exposed an incorrect AgentManager object path; that defect was fixed in `ab6b577f773e650a930d07064adcd8556cb55248` before this successful acceptance run.

## Initiator-side evidence

While the `debiancb1` receive window was open, `arisu` ran:

```bash
cargo run -p blueroute-linux --example bluez_pair_probe --locked -- debiancb1
```

One initial discovery attempt did not observe `debiancb1` within the 10-second discovery window. A subsequent attempt discovered the known Linux test node and reported its pre-pairing state as `paired=false` and `trusted=false`.

The Rust probe then invoked the BlueRoute pairing adapter, explicitly enabled BlueZ trust, refreshed the Device1 state, and completed successfully:

```text
target: <BlueZ Device1 path> name=debiancb1 paired=false trusted=false
pairing complete: paired=true trusted=true
```

The concrete Bluetooth address embedded in the BlueZ object path is intentionally omitted from this public evidence record.

## Acceptance result

P4-004 acceptance is satisfied:

- the receiving Linux node ran BlueRoute's Rust-controlled incoming pairing agent;
- the initiating Linux node discovered the receiver through the Rust adapter;
- the peer began unpaired and untrusted;
- the Rust pairing path completed successfully;
- the initiator's authoritative post-pairing Device1 refresh reported `paired=true` and `trusted=true`.

Therefore two Linux test nodes completed pairing through the Rust-controlled BlueRoute flow without using `bluetoothctl` or a graphical pairing agent for the pairing operation.

# P7-003 — Independent local control-key comparison (software-only)

**Status:** Partial operator-facing identity verification. Qualified on `master` at `edcb04dded64308d17af57378f9e440e765bc448` with [exact-head CI #39917758815](https://github.com/ekkus93/blueroute/actions/runs/39917758815) passing. **No new peer enrollment or production join is enabled.**

## Read the candidate machine's own public identity

Run on the machine whose local control TLS identity you want to inspect:

```sh
blueroute peer local-key
blueroute --json peer local-key
```

The output includes the daemon's local `NodeId`, canonical 64-hex-character SPKI SHA-256 fingerprint and its **current** `NetworkId` (or `null` if not a network member). These are local daemon observations, **not** evidence of a remote peer's private-key possession or an approved network join.

## Compare an independently supplied candidate identity

The operator must obtain the candidate `NodeId` and SPKI fingerprint through a separate channel and compare them **on the candidate machine itself**:

```sh
blueroute peer verify-local-key <NodeId> <64-character-SPKI-SHA256>
```

For a daemon **already** in the intended network, additionally check its current network against the independently supplied NetworkId:

```sh
blueroute peer verify-local-key <NodeId> <64-character-SPKI-SHA256> --network <NetworkId>
blueroute --json peer verify-local-key <NodeId> <64-character-SPKI-SHA256> --network <NetworkId>
```

A prospective PANU may not have joined yet. In that case **omit** `--network`; the CLI verifies only its local NodeId and SPKI fingerprint and reports `network_verified: false` in JSON. Never interpret that result as confirmation of the host's network identity. Independently verify the intended network on the host, then require authenticated enrollment to bind the candidate key to that network.

The command rejects noncanonical fingerprints, zero identities, missing daemon key material, mismatched local identity, and any disagreement between the daemon's status and diagnostics network observations. A successful JSON response contains `matches: true` and `enrollment_performed: false`; a failed comparison returns a nonzero exit status. A successful local comparison **does not write durable pins, grant trust, initiate TLS, or authorize membership**.

## Security and integration boundary

- The comparison must be initiated by a human who actually obtained the reference values independently. Repeating the same untrusted Bluetooth advertisement, TCP certificate preface or peer claim in two places is **not** independent confirmation.
- The CLI is read-only and uses the existing version-checked daemon D-Bus API. Its observations are snapshots, not a signed transcript. It does not prove remote private-key possession.
- The existing library `enroll_confirmed_control_candidate` separately requires durable P6-003 approval, a matching independently confirmed fingerprint, a **fresh** pinned mutual-TLS 1.3 handshake, exporter-bound Hello/HelloAck and a bounded confirmation deadline before it persists a pin.
- A complete operator enrollment flow still needs remote candidate discovery/presentation, explicit PolicyKit-authorized confirmation, secure handoff to the daemon enrollment primitive, live PANU/TLS session lifecycle and rollback. **Production `JoinNetwork` remains fail-closed.**
- No physical Bluetooth, multi-machine join, or throughput acceptance was performed; see `docs/HARDWARE_ACCEPTANCE_BACKLOG.md`.

## Deterministic software evidence

The CLI unit tests cover canonical input parsing, optional/required network modes, network drift between daemon status and diagnostics, missing key material, wrong NodeId/fingerprint, zero identities, pre-join identity-only success and explicit non-enrollment output. The exact-head CI run includes locked formatting/build/Clippy, Rust tests and existing D-Bus/socket/package integration checks. These do not substitute for an independent human confirmation or physical PAN test.

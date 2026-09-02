# P5 — Local Daemon Protocol

BlueRoute front ends communicate with the local daemon through the versioned `blueroute-protocol` contract. The semantic command, response, and event types remain independent of D-Bus so the same shared types can be tested without a bus and can be reused by every front end.

## API versioning

The v1 local service uses:

- service: `org.blueroute.Service1`
- object: `/org/blueroute/Service1`
- interface: `org.blueroute.Service1`
- current semantic API version: `1.0`

Major versions are incompatible. Minor versions are additive. A client may use a server with the same major version when the client's minor version is less than or equal to the server's minor version. P5-005 is responsible for enforcing this negotiation before normal commands are sent.

## Payload encoding

Commands, responses, and events use compact UTF-8 JSON produced by `serde_json::to_string`. The public helpers are:

- `encode_command` / `decode_command`
- `encode_response` / `decode_response`
- `encode_event` / `decode_event`

Protocol enums use an explicit snake-case `type` discriminator and a `data` field where a variant has payload data. For example:

```json
{"type":"create_network","data":{"name":"Lab network"}}
```

Stable identifiers are encoded as their canonical 32-character lowercase hexadecimal strings rather than implementation-specific byte arrays. `DisplayName` is decoded through its domain validator, so malformed or whitespace-only names do not bypass the same validation applied to locally constructed values.

Only protocol-facing core domain types implement serde. The wire format intentionally contains no D-Bus object paths, BlueZ types, NetworkManager types, or front-end-specific types.

## Determinism and fail-closed parsing

The protocol crate tests that every command variant round-trips, representative responses and all event categories round-trip, repeated encoding of the same value is byte-for-byte identical, malformed JSON is rejected, unknown command variants are rejected, malformed stable identifiers are rejected, and invalid display names are rejected during deserialization.

This JSON layer is the payload contract carried by the P5 D-Bus service. D-Bus remains responsible for local transport, service ownership, signals, and authorization; the JSON codec keeps the semantic protocol deterministic and testable independently of that transport.

## CI acceptance

GitHub Actions run `33579043013` passed the normal repository gates with the serialization implementation in place: formatting, locked workspace check, Clippy with `-D warnings`, and the complete locked workspace test suite all succeeded. The subsequent TODO closeout restored the normal read-only CI workflow before the final merge candidate was validated again.

## Reserved gateway behavior

`SetInternetSharing` and gateway-related events remain represented in the protocol for forward compatibility, but the current daemon must reject Internet-sharing mutations until the gateway phase implements them. Presence in the wire schema is not an authorization or feature-enablement signal.

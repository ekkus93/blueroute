# P7-002 — Control protocol envelope

Implements the envelope defined by ADR 0001 in `crates/blueroute-protocol/src/control.rs`. It is transport- and crypto-free: frames are carried inside an authenticated Noise session (later P7 work), and nothing here is trusted until the session has authenticated the peer.

## Wire format

`u32_be(length) || JSON(ControlEnvelope)`

| Field | Meaning |
| --- | --- |
| `version` | `{major, minor}`. Majors are incompatible; a newer minor within the same major is accepted. |
| `network` | `NetworkId` the message belongs to. |
| `sender` | `NodeId` of the sender. |
| `sequence` | Strictly increasing per sender and session. |
| `message_type` | `[a-z0-9_.-]`, 1..=32 bytes. |
| `critical` | If true and the type is unknown, the session must be rejected. |
| `payload` | Opaque JSON, bounded by the frame limit. |

## Limits and rejection rules

- Frame body is limited to 16 KiB. `frame_body_len` checks the 4-byte header first, so a hostile peer cannot force a larger allocation.
- Rejected with typed errors: oversized, truncated, trailing bytes, malformed JSON, invalid message-type name, unsupported major version, unknown critical type, wrong network, wrong sender, non-increasing sequence.
- Unknown non-critical types are ignored, so minor-version additions do not break older peers.
- `InboundValidator` is created per authenticated session, bound to the session's network and peer `NodeId`. Sequence state advances only when a message is accepted, so a rejected message cannot desynchronize the session.

## Freshness and replay

Replay across sessions is prevented by the Noise AEAD. Within a session, the per-sender sequence rejects replays and reordering. Credentials carry time bounds (ADR 0001).

## Not in scope

Handshake, credentials, and the hello payload are P7-003/P7-004. Property and fuzz coverage is P7-005.

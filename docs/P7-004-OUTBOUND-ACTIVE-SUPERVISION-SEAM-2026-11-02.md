# P7-004 — Outbound PANU active-supervision seam (2026-11-02)

## Scope

The outbound PANU bootstrap already performed pre-dial and post-TLS/Hello checks. A registered TLS owner still needs periodic revalidation throughout its active lifetime. This change adds an exported, synchronous, fail-closed **supervision entry point** without enabling production joining or claiming an automatic supervisor exists.

`supervise_registered_panu_control_session` accepts the existing registered TLS owner, original PANU link and IPv4 lease, durable trust operations and expected pinned remote identity. Each invocation:

1. Rejects a mismatch between the registered TLS peer/network and the independently pinned expected peer/network.
2. Revalidates the deterministic PANU control endpoint and durable TLS authorization before backend queries.
3. Re-observes BlueRoute-owned IPv4 addresses and active interface/prefixes through NetworkManager, rejecting stale, foreign or ambiguous source ownership.
4. Re-observes the exact original BlueZ PANU attachment, rejecting missing, replaced or mismatched peer/interface.
5. Rechecks the enrolled peer key and active TLS approval after backend observations.
6. On any failure, invokes `RegisteredControlSession::cancel`, which closes the owned TLS socket before retiring its exact registry generation. A valid tick retains the owner.

The implementation deliberately does not accept a raw socket or peer-claimed identity as authorization.

## Commits and verification

- Implementation: `f62676156ba958e05d4708635c52270e7c0daff1`.
- Rust formatting: `233dc8bae4844c44d85b103a9138c0154cab7289`.
- Public daemon library export: `86903e7acedaacefa5282783cddccfb0a69920e8`.
- Exact-head CI: [run 41540397862](https://github.com/ekkus93/blueroute/actions/runs/41540397862), **passed** (formatting, locked compilation, Clippy, Rust tests, integration checks).
- A deterministic cancellation-contract regression checks fail-closed owner retirement for address-ownership loss, PAN loss and authorization revocation, and retention on successful validation. This does **not** simulate the full BlueZ/NetworkManager runtime or a real Bluetooth adapter.

## Outstanding acceptance

The daemon still needs to own the outbound registered session, invoke this check on a bounded recurring tick, subscribe to exact-attachment loss, cancel and reconnect using generation-scoped backoff, and integrate verified key enrollment and authenticated JoinNetwork. A successful tick cannot prevent ownership changes immediately afterward. Blocking backend calls require bounded/cancellable orchestration. Physical multi-node Bluetooth acceptance remains deferred. **Do not mark P7-003, P7-004, P7-005 or P6-004 complete on this evidence.**

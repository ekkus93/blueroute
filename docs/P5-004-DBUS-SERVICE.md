# P5-004 — Daemon D-Bus Service Skeleton

P5-004 turns `blueroute-daemon` into the owner of the versioned local BlueRoute D-Bus API defined by `blueroute-protocol`.

## Production service

The daemon serves the existing v1 contract on the system bus:

- service: `org.blueroute.Service1`
- object: `/org/blueroute/Service1`
- interface: `org.blueroute.Service1`

Startup remains fail-closed for durable identity and membership state. After loading durable state, the daemon runs the P4-010 system capability probe and exposes that live capability snapshot through the service. The support classification is mapped to the current daemon health summary:

- fully supported / client-only -> `Healthy`;
- degraded -> `Degraded`;
- unsupported -> `Error`.

The process then requests the well-known D-Bus name, registers the service object, and remains alive to serve requests. P5-006 owns systemd packaging/startup policy. P5-007 owns the final system-bus authorization/Polkit policy; P5-004 does not add a permissive authorization fallback.

## Methods

The skeleton exposes:

- `Version() -> (u16, u16)` for transport-level version negotiation;
- `Status() -> String`, containing a serialized `Response::Status`;
- `Capabilities() -> String`, containing a serialized `Response::Capabilities`;
- `Request(String) -> String`, decoding the stable P5 JSON command payload.

`Request` currently implements `GetStatus` and `GetCapabilities`. Other well-formed commands return D-Bus `NotSupported` until their owning tasks implement them. Malformed command JSON returns D-Bus `InvalidArgs`; malformed input is never treated as a default command and does not silently fall through to another operation.

## Events

`DaemonService` defines the `Event(String)` D-Bus signal. `emit_event` accepts the typed shared `blueroute_protocol::Event`, serializes it through the deterministic P5 codec, resolves the registered service interface, and emits the signal. This keeps event producers typed while D-Bus carries the same stable payload format used by all future front ends.

## Bus-level acceptance

`apps/daemon/tests/dbus_service.rs` is an ignored-by-default integration test because it requires an isolated D-Bus session. CI explicitly runs it with `dbus-run-session`.

The test uses two real zbus connections through the broker. It verifies that a client can:

1. resolve the well-known BlueRoute service;
2. query the API version;
3. obtain and decode status;
4. obtain and decode capabilities;
5. submit malformed request data and receive an error rather than a fallback response;
6. subscribe to `Event` and receive/decode a typed health event emitted by the daemon service.

The event wait is bounded so a broken signal path fails instead of hanging CI indefinitely.

## CI evidence

GitHub Actions run `33580417946` passed on the P5-004 merge candidate, including:

- `cargo fmt --all -- --check`;
- `cargo check --workspace --locked`;
- `cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `cargo test --workspace --locked`;
- the isolated real-broker D-Bus service/query/signal integration test.

## Result

P5-004 acceptance is satisfied: the test client queries the daemon service over D-Bus and receives a daemon event over the D-Bus signal path. No shell-output parsing, silent request fallback, or permissive authorization fallback was introduced.

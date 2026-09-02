# P5-005 — Reusable BlueRoute Daemon Client

P5-005 implements `blueroute-client` as the single reusable system-D-Bus client for BlueRoute front ends. CLI, TUI, and Tauri code can depend on this crate instead of duplicating service discovery, API negotiation, payload encoding, event decoding, or daemon-restart handling.

## Connection and version negotiation

`BlueRouteClient::connect()` opens the system D-Bus and immediately queries `Version()` on `org.blueroute.Service1`. `from_connection()` provides the same version-gated construction for deterministic tests or callers that already own a zbus connection.

A client is created only when `ApiVersion::CURRENT` is compatible with the daemon-reported version. Different major versions and server versions older than the client-required minor version fail with `ClientError::IncompatibleVersion`.

Version negotiation is not only a connect-time check. Every normal command re-runs the read-only `Version()` query immediately before sending `Request`. This matters across daemon replacement: a long-lived client cannot silently continue normal commands against a new owner of the well-known service name when that replacement exposes an incompatible API.

## Typed requests

`request(&Command)` uses the shared deterministic P5 JSON codec and returns a typed `Response`. Convenience methods currently include:

- `status() -> DaemonStatus`;
- `capabilities() -> NodeCapabilities`.

If the daemon returns the wrong response variant, the client fails with `UnexpectedResponse`; it does not reinterpret or coerce the response.

## Events

`events()` subscribes to the daemon's `Event` signal and returns an `EventSubscription`. `next_event()` decodes the D-Bus string through the shared protocol event codec.

The subscription rechecks the daemon API version before accepting each event. A D-Bus signal subscription can survive a well-known-name owner change, so this prevents an event from a replacement incompatible daemon from bypassing the same compatibility gate used for commands.

## Reconnect behavior

`reconnect(timeout)` retries only the daemon's read-only `Version()` method until a compatible owner is available or the bounded timeout expires. Once a compatible daemon appears, the client records its version and can resume explicit caller-issued operations.

BlueRoute deliberately does **not** auto-replay normal commands during reconnect. If a mutating call reached the old daemon but its reply was lost, automatic replay could duplicate a side effect. Higher-level code must decide whether and how an operation is safe to retry.

An incompatible replacement daemon fails immediately rather than being treated as transient unavailability.

## Real-broker acceptance

`crates/blueroute-client/tests/dbus_client.rs` is an ignored-by-default integration test that CI executes under `dbus-run-session`. The test uses real zbus connections through an isolated broker and proves the full client contract:

1. connect to a compatible BlueRoute daemon service;
2. negotiate API version before returning the client;
3. query and decode typed status and capabilities;
4. subscribe to and decode a typed daemon event;
5. release the first daemon owner, start a compatible replacement owner, reconnect, and query the replacement state through the existing client connection;
6. replace that daemon with an intentionally incompatible v2 service;
7. verify the existing client rejects the incompatible daemon with `IncompatibleVersion`;
8. verify the incompatible service's `Request` method invocation counter remains exactly zero.

Step 8 is the acceptance proof for P5-001: incompatibility is detected **before normal commands**.

## CI evidence

GitHub Actions run `33581043131` passed on the P5-005 merge candidate, including:

- `cargo fmt --all -- --check`;
- `cargo check --workspace --locked`;
- `cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `cargo test --workspace --locked`;
- the existing P5-004 real-broker daemon D-Bus integration test;
- the P5-005 real-broker reusable-client integration test.

## Result

P5-005 acceptance is satisfied. The reusable client owns D-Bus connection/version/request/event/reconnect mechanics, and P5-001 acceptance is also satisfied because incompatible daemon replacement is proven to fail closed before a normal request is delivered.

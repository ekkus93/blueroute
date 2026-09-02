# P4-011 — Network Backend Contract Tests

P4-011 makes the Linux network-backend boundary testable independently of NetworkManager. The production traits remain `NetworkStateBackend` and `IpNetworkBackend`; this task adds a deterministic black-box contract suite around those public traits rather than introducing NetworkManager types into core topology code.

## Contract exercised

`crates/blueroute-linux/tests/network_backend_contract.rs` defines a reusable contract function over any type implementing both public backend traits. A clean backend fixture must satisfy the following behavior:

- bridge creation is idempotent for the same owner/interface;
- the resulting connection retains explicit BlueRoute ownership;
- address application is idempotent and does not duplicate owned state;
- route application is idempotent;
- a stale route variant with the same destination is reconciled instead of accumulated;
- a network-state subscription exposes the current owned connection snapshot;
- repeated forwarding requests are safe;
- route, address, and interface cleanup are idempotent;
- cleanup leaves no state owned by the removed fixture;
- cross-owner takeover/removal fails closed and preserves the other owner's state.

The deterministic fake backend executes the complete contract in CI. A future backend can be added to the same test harness by providing a clean deterministic fixture implementing `NetworkStateBackend + IpNetworkBackend`; the contract assertions do not depend on NetworkManager object paths, D-Bus types, or `nmcli` output.

## NetworkManager conformance

The integration test contains a compile-time conformance assertion that `NetworkManagerBackend` implements the same `NetworkStateBackend + IpNetworkBackend` boundary. NetworkManager's backend-specific deterministic unit tests continue to cover profile ownership metadata, fail-closed malformed ownership, idempotent address mutation, route reconciliation, preservation of unrelated routes, cross-family validation, duplicate-owned-profile rejection, deterministic event diffs, and Linux interface validation.

Live D-Bus behavior is already covered by the P4-007 and P4-008 hardware acceptance runs. P4-011 therefore does not require a live NetworkManager daemon in CI and does not add a silent fallback when D-Bus is unavailable.

## Architecture guard

The contract integration test also checks the architectural boundary directly:

- `blueroute-core/Cargo.toml` may not depend on `blueroute-linux` or `zbus`;
- core and topology source may not import `NetworkManagerBackend`, `zbus` types, or the NetworkManager D-Bus service name.

The core capability/configuration model may still contain backend-neutral enum values such as `NetworkBackend::NetworkManager`; those values describe capability/configuration and are not Linux adapter implementation types.

## CI acceptance

GitHub Actions run `33578415335` passed the reusable contract suite together with formatting, locked workspace check, Clippy with `-D warnings`, and the complete workspace test suite. The closeout step then updated `docs/TODO.md` and restored the normal read-only CI workflow; it made no production-code changes.

## Acceptance

P4-011 is complete when the reusable fake-backend contract suite, NetworkManager trait conformance assertion, architecture guard, formatting, workspace check, Clippy, and full locked workspace tests all pass in CI. Those conditions are satisfied.
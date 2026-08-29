# BlueRoute Development Guide

## Baseline

The initial development and packaging baseline is Debian 13 with BlueZ, NetworkManager, systemd, and system D-Bus. This is a practical baseline, not a statement that BlueRoute's architecture is Debian-only.

The initial Rust toolchain is pinned in `rust-toolchain.toml` to Rust 1.95.0. Cargo manifests declare a minimum Rust version of 1.95.

## Debian packages

For the current Rust-only workspace baseline, install the normal build tools plus the runtime services BlueRoute will integrate with:

```bash
sudo apt update
sudo apt install -y \
  build-essential \
  pkg-config \
  bluez \
  network-manager \
  dbus \
  systemd
```

Future Linux adapter or Tauri work may add development libraries. Add them only when the implementation actually requires them, and keep this document synchronized with CI/package metadata.

## Rust toolchain

Install `rustup`, then enter the repository. Rustup will use `rust-toolchain.toml`; alternatively install the pinned toolchain explicitly:

```bash
rustup toolchain install 1.95.0 --profile minimal --component rustfmt,clippy
```

Verify:

```bash
rustc --version
cargo --version
cargo fmt --version
cargo clippy --version
```

## Required checks

Run these before committing Rust changes:

```bash
cargo fmt --all -- --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Formatting problems can be fixed with:

```bash
cargo fmt --all
```

## Frontend toolchain

Node.js is not required for the current placeholder desktop crate. When the Tauri frontend is introduced, BlueRoute will pin an explicit supported Node.js release and package-manager version in repository metadata and CI. Until that task begins, contributors should not infer a Node requirement from their local environment.

## Test classes

### Deterministic tests

These must be suitable for ordinary CI whenever feasible:

- domain identifiers and state machines;
- topology and route algorithms;
- addressing policy;
- protocol serialization/parsing;
- persistence migrations;
- adapter translation using fakes/mocks;
- CLI/UI behavior driven by a fake daemon.

### Linux integration tests

Use namespaces, virtual interfaces, fake D-Bus services, or controlled system services when practical. These tests may require a Linux runner but should avoid physical radio requirements unless the behavior under test is inherently hardware-dependent.

### Physical Bluetooth acceptance

Real PAN/BNEP behavior, controller connection limits, throughput, range, suspend/resume, and adapter quirks require physical hardware. Record exact evidence under `docs/platforms/` or `docs/hardware/`. A passing CI job is never a substitute for these tests.

## Repository layout

- `crates/blueroute-core`: hardware-independent domain logic.
- `crates/blueroute-protocol`: shared API/control protocol types.
- `crates/blueroute-linux`: Linux-specific adapters.
- `crates/blueroute-client`: reusable daemon client.
- `apps/daemon`: background service.
- `apps/cli`: command-line client.
- `apps/tui`: terminal UI.
- `apps/desktop`: desktop/Tauri application boundary.
- `docs/`: product, architecture, development, and test evidence.

The placeholder crates intentionally contain almost no behavior. Later TODO phases add functionality behind these boundaries.

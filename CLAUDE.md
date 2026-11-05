# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

BlueRoute builds ordinary TCP/IP networks over Bluetooth PAN/BNEP (Rust workspace: shared core + D-Bus daemon; CLI/TUI/Tauri are front ends). Multi-hop uses normal Linux IP routing, not Bluetooth Mesh. Architecture and invariants: `docs/SPEC.md`. Task backlog and status (authoritative): `docs/TODO.md`. Setup: `docs/DEVELOPMENT.md`.

## Required checks (CI runs all of these, always with `--locked`)

```
cargo fmt --all -- --check        # fix with: cargo fmt --all
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

- Keep `Cargo.lock` in sync; a stale lockfile fails CI.
- Toolchain is pinned to Rust 1.95.0, edition 2024.
- Some D-Bus integration tests are `#[ignore]`d, so plain `cargo test` skips them. Run them with `dbus-run-session`, `--ignored --exact`, and the full test name, e.g.:
  `dbus-run-session -- cargo test -p blueroute-daemon --test dbus_service --locked dbus_service_round_trip_and_event -- --ignored --exact`
  See `.github/workflows/ci.yml` for the three CI invocations (daemon `dbus_service`, daemon `authorization`, client `dbus_client`).
- Packaging files in `packaging/` (systemd, D-Bus conf, polkit) are validated in CI; the polkit policy must contain exactly `org.blueroute.modify` and `org.blueroute.internet-sharing`.

## Code rules

- `unsafe_code` is forbidden; `dbg!` is denied; clippy warnings are errors. rustfmt `max_width = 100`.
- zbus is built with `default-features = false` + `async-io`. No tokio; use async-io / futures-lite.
- Front ends must not implement their own Bluetooth or routing logic. Hardware-specific behavior goes behind capability and `blueroute-linux` adapter boundaries.
- Changing an invariant in `docs/SPEC.md` means updating the spec first or in the same change.
- Prefer deterministic tests needing no hardware (fakes, mocks, fake D-Bus, namespaces).

## Workflow conventions

- Keep each change scoped to the smallest sensible TODO task. Reference the task ID (e.g. `P4-003`) in commits, PRs, and evidence.
- Don't mark a task complete unless its acceptance criteria are met. Status markers: `[ ]`, `[-]`, `[x]`, `[!]`.
- Branches: `P<phase>-<nnn>_<snake_description>`; default branch is `master`.
- Commits: lowercase Conventional Commits (`feat:`, `fix:`, `test:`, `docs:`, `style:` for fmt-only, `build:`, `chore:`), short and imperative.
- Terminology is strict: Supported, Tested, Experimental, Unsupported. Never generalize one adapter's result into a global claim (peer limit, throughput).
- Physical Bluetooth testing is manual; record evidence in `docs/` (computer, adapter/controller, distro, kernel, BlueZ version, network backend, exact procedure). CI is never a substitute.

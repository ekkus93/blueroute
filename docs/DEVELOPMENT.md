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

Building the whole workspace also compiles the Tauri 2 desktop shell (`apps/desktop/src-tauri`), which needs the GTK/WebKit development libraries (the same list CI installs):

```bash
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libsoup-3.0-dev \
  librsvg2-dev \
  libayatana-appindicator3-dev
```

Keep this list synchronized with `.github/workflows/ci.yml` and package metadata.

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

The desktop frontend (React + TypeScript, built with Vite) lives in `apps/desktop`; the Tauri 2 Rust shell is `apps/desktop/src-tauri`. Only the desktop frontend needs Node.js; the daemon, CLI and TUI do not.

- Node.js is pinned to **26.13.0** (`.node-version`, `engines` in `apps/desktop/package.json`) and npm to **11.20.0** (the npm bundled with that Node release). `.npmrc` sets `engine-strict=true` and exact saves; CI verifies both versions.
- Dependencies are pinned exactly and locked by `apps/desktop/package-lock.json`. Always install with `npm ci`.
- Run from `apps/desktop`:

```bash
npm ci
npm run format:check   # prettier (npm run format to fix)
npm run lint           # oxlint; warnings fail the run
npm run typecheck
npm test               # vitest
npm run build          # type-check + vite build into ui/dist
npx tauri build --debug --no-bundle   # build the desktop binary without packaging
```

The linter is [oxlint](https://oxc.rs) (pinned in `package.json`, configured in `.oxlintrc.json`), chosen because typescript-eslint does not support TypeScript 7, which has no JavaScript compiler API. Correctness rules are errors, suspicious ones warnings (`--deny-warnings` fails on either), and the React hooks rules (`rules-of-hooks`, `exhaustive-deps`) are errors. **No rule is disabled or excepted:** findings are fixed in the code. Consequences worth knowing:

- The frontend uses React's classic JSX transform (`"jsx": "react"`, `jsxRuntime: "classic"` in Vite), and every JSX file imports `React`, as `react/react-in-jsx-scope` requires.
- The compiler-based effect rules cannot analyze `try/finally`, so effects and memoized callbacks avoid `finally`; `useDaemon` uses a never-throwing helper instead.
- Status regions use `<output>` and grouped controls `<fieldset>` rather than explicit `role` attributes.
- Every `vi.fn` mock is typed after the interface member it replaces.
- The stylesheet is linked from `ui/index.html`, not imported for its side effect.

To run the app against the daemon on this machine, use `npm run tauri dev` from `apps/desktop` (it starts the Vite dev server and opens the window).

`packaging/debian/build-package.sh` builds two packages: `blueroute` (daemon, CLI, TUI) and `blueroute-desktop` (the app, launcher and icon). Building them needs the pinned Node.js as well as Rust, because the app embeds its built frontend; it also needs `dpkg-dev` for `dpkg-shlibdeps`.

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

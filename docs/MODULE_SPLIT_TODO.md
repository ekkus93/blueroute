# Module Split TODO

**Status:** NOT STARTED; planning only.  
**Baseline:** `1870e406836174c0e0433f42d026f71e497b73f5`.  
**Companion specification:** [MODULE_SPLIT_SPEC.md](MODULE_SPLIT_SPEC.md).  
**Implementation evidence:** `MODULE_SPLIT_EVIDENCE.md` (created by MS-BASE).

## Tracking

- Stable `MS-*` IDs. `[ ]` pending, `[-]` in progress, `[x]` accepted, `[!]` blocked with a reason.
  Keep parents and children consistent; tick an item only when its acceptance is evidenced.
- Each split is a pure move (SPLIT-1). If a behavior change turns out to be needed, stop, record
  why, and handle it as its own reviewed commit rather than folding it into a move.
- Temporary listings, help-text captures and doc snapshots go in the ignored workspace `tmp/`;
  the durable record is the evidence file.
- Commit directly to master with Conventional Commits scoped to the task (`refactor(MS-CLI): …`),
  run `cargo fmt --all` first, push, and verify CI on the exact pushed head with `gh`.

## Task register

| Task | Outcome | Dependencies | State |
| --- | --- | --- | --- |
| MS-BASE | Baseline listings, help text and public-surface snapshots | None | `[ ]` |
| MS-CLI | `apps/cli/src/main.rs` split into args, format, local_key, failure, run | MS-BASE | `[ ]` |
| MS-NM | `networkmanager.rs` split into dbus, inventory, ownership, profile, addresses, routes | MS-BASE | `[ ]` |
| MS-TLS | `control_tls.rs` split into identity, certificate, enrollment, transport, session | MS-BASE | `[ ]` |
| MS-BLUEZ | Pairing agent moved out of `bluez.rs` | MS-BASE | `[ ]` |
| MS-VALIDATE | Exact-candidate validation, packages and CI | all splits | `[ ]` |
| MS-CLOSEOUT | Review, requirement map and closure | MS-VALIDATE | `[ ]` |

Order by risk: CLI (no shared API), NetworkManager (crate-private, ownership-critical), control
TLS (security-critical, re-exported), BlueZ (smallest gain). Splits are independent of each other,
but do them one at a time so each diff stays reviewable.

## MS-BASE — Record the baseline

**Requirements:** SPLIT-2, SPLIT-4, VERIFY-1/2.

- [ ] **MS-BASE-01:** Record HEAD, dirty state and toolchain. Confirm the four files' production
  line counts and responsibilities against the specification table.
- [ ] **MS-BASE-02:** Capture sorted test-name listings: `cargo test -p blueroute --bins --locked
  -- --list`, `cargo test -p blueroute-linux --lib --locked -- --list` and `cargo test -p
  blueroute-daemon --lib --locked -- --list`. Expected totals 22, 153 and 343 (14
  `networkmanager::`, 15 `bluez::`, 22 `control_tls::`).
- [ ] **MS-BASE-03:** Capture `blueroute --help` and `--help` for every subcommand and nested
  subcommand, and the `all.html` item lists from `cargo doc --no-deps --locked -p blueroute-linux
  -p blueroute-daemon`.
- [ ] **MS-BASE-04:** Create `docs/MODULE_SPLIT_EVIDENCE.md` with the baseline counts, and record
  how each later comparison is made so it can be repeated without the `tmp/` captures.

**Acceptance:** Baseline names, help text and public items are captured and their totals recorded.

## MS-CLI — Split the CLI entry point

**Requirements:** SPLIT-1..7, VERIFY-1.

- [ ] **MS-CLI-01:** Move the command tree and parsing to `args.rs`, the formatters to
  `format.rs`, local-key verification to `local_key.rs`, `CliFailure` to `failure.rs` and `run` to
  `run.rs`, leaving `main.rs` with module declarations and `main`. Keep `error_guidance.rs`.
- [ ] **MS-CLI-02:** Move each of the 22 tests next to the code it exercises; no test changes
  beyond its module path.
- [ ] **MS-CLI-03:** Verify: fmt, Clippy, workspace tests, `cli_exit_contract` broker test, the 22
  test names, and every help text byte-identical to MS-BASE. Check moved blocks with
  `--color-moved` and account for every non-moved line.

**Acceptance:** CLI behavior, help and output are unchanged; `main.rs` holds only `main`; each new
module has a one-responsibility doc comment.

## MS-NM — Split the NetworkManager backend

**Requirements:** SPLIT-1..7, VERIFY-1/2.

- [ ] **MS-NM-01:** `git mv` the file to `networkmanager/mod.rs`, then move items into `dbus.rs`,
  `inventory.rs`, `ownership.rs`, `profile.rs`, `addresses.rs` and `routes.rs` per the
  specification. `NetworkManagerBackend`'s trait impls stay in `mod.rs` as thin delegations.
- [ ] **MS-NM-02:** Keep the fail-closed ownership rules together in `ownership.rs`: owner
  parsing, exactly-one-owned-profile, foreign interface/active-connection rejection and profile
  UUID derivation. Nothing outside `networkmanager` gains access to them.
- [ ] **MS-NM-03:** Move the 14 tests to their submodules (ownership, address, route and event
  tests separately).
- [ ] **MS-NM-04:** Verify: fmt, Clippy, workspace tests, the 153 `blueroute-linux` test names,
  the `blueroute_linux` public items, the daemon's broker suites, and the moved-block check.

**Acceptance:** Same public surface and tests; ownership logic is reviewable in one module; no
behavior change.

## MS-TLS — Split the control TLS module

**Requirements:** SPLIT-1..7, VERIFY-1/2.

- [ ] **MS-TLS-01:** `git mv` the file to `control_tls/mod.rs`, then move items into
  `identity.rs`, `certificate.rs`, `enrollment.rs`, `transport.rs` and `session.rs` per the
  specification. `mod.rs` re-exports exactly the items the daemon's `lib.rs` re-exports today.
- [ ] **MS-TLS-02:** Keep private key material, the identity record format and the verifier
  internals private to their submodules; confirm the key-wiping `Drop` and the bounded enrollment
  I/O move unchanged.
- [ ] **MS-TLS-03:** Move the 22 tests, including the real localhost TLS handshakes, to the
  submodules they exercise.
- [ ] **MS-TLS-04:** Verify: fmt, Clippy, workspace and experimental-join tests, the 343 daemon
  test names, the `blueroute_daemon` public items, every daemon broker suite, and the moved-block
  check.

**Acceptance:** Same public surface and tests; identity, pinning, enrollment, transport and session
code each live in one module; no behavior change.

## MS-BLUEZ — Move the pairing agent out of the BlueZ backend

**Requirements:** SPLIT-1..7, VERIFY-1/2.

- [ ] **MS-BLUEZ-01:** `git mv` the file to `bluez/mod.rs` and move the pairing control, permit,
  agent, incoming-pairing, pair and trust items to `bluez/pairing.rs`. Keep `BluezBackend`'s
  `pub(crate)` fields and the `crate::bluez::BluezBackend` path that `pan.rs` uses.
- [ ] **MS-BLUEZ-02:** Move the pairing-related tests with the code; keep the rest in `mod.rs`.
- [ ] **MS-BLUEZ-03:** Verify: fmt, Clippy, workspace tests, the 153 `blueroute-linux` test names
  (15 under `bluez::`), the public items, and the moved-block check.

**Acceptance:** Pairing is its own module; discovery and signals stay in `mod.rs`; no behavior
change.

## MS-VALIDATE — Validate the exact candidate

**Requirements:** VERIFY-3, SPLIT-2/4/5.

- [ ] **MS-VALIDATE-01:** Run the full command reference below on the final candidate; record
  counts, ignored cases and any change from the baseline totals (there should be none).
- [ ] **MS-VALIDATE-02:** Build both Debian packages, record hashes, and run the source and
  extracted-script lifecycle checks and the isolated upgrade.
- [ ] **MS-VALIDATE-03:** Confirm with `gh` that CI passed on the exact pushed candidate and ran
  every broker suite.

**Command reference** (repository root, Rust 1.95.0):

```bash
cargo fmt --all -- --check
cargo build --workspace --locked
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p blueroute-daemon --features experimental-join --locked
cargo clippy -p blueroute-daemon --all-targets --features experimental-join --locked -- -D warnings
python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v
bash packaging/debian/test-maintainer-lifecycle.sh
bash packaging/debian/build-package.sh
cargo build --release --locked -p blueroute-linux --example state_check
```

Plus every private-broker suite listed in `CLAUDE.md` and `ci.yml`, the frontend checks in
`apps/desktop` (`npm ci`, format, lint, typecheck, test, build), `bash
packaging/debian/test-package-upgrade.sh <core.deb> target/release/examples/state_check`, and the
lifecycle check with `BLUEROUTE_MAINTAINER_SCRIPT_DIR` pointing at the extracted control scripts.

**Acceptance:** Every check passes on the exact candidate with baseline test totals; CI passes.

## MS-CLOSEOUT — Review and close

**Requirements:** VERIFY-4, all SPLIT requirements.

- [ ] **MS-CLOSEOUT-01:** Independent review of each split diff for non-move edits, visibility
  widening and lost tests, with a specific re-read of the moved ownership and identity/verifier
  code. Resolve findings before acceptance.
- [ ] **MS-CLOSEOUT-02:** Record per-module production line counts and doc comments; justify any
  module over about 600 lines.
- [ ] **MS-CLOSEOUT-03:** Decide, with a short written reason, whether `apps/daemon/src/lib.rs` and
  `crates/blueroute-client/src/lib.rs` need a follow-up split; do not split them in this backlog.
- [ ] **MS-CLOSEOUT-04:** Map every SPLIT and VERIFY requirement to tasks and evidence, then mark
  the backlog COMPLETE. Report existing qualification gates as still open and separate.

**Acceptance:** Reviewed, behavior-preserving splits with identical public surface and tests;
evidence complete.

## Requirement ledger

| Requirement | Tasks | Required evidence | State |
| --- | --- | --- | --- |
| SPLIT-1 move only | MS-CLI/NM/TLS/BLUEZ, MS-CLOSEOUT-01 | moved-block checks; review | PENDING |
| SPLIT-2 public surface | MS-BASE-03, each split, MS-VALIDATE | identical `all.html` items and help text; dependents unedited | PENDING |
| SPLIT-3 minimal visibility | each split, MS-CLOSEOUT-01 | review of every visibility change | PENDING |
| SPLIT-4 tests travel | MS-BASE-02, each split | identical test-name multisets and totals | PENDING |
| SPLIT-5 invariants | MS-NM-02, MS-TLS-02, MS-VALIDATE | review; full validation | PENDING |
| SPLIT-6 one responsibility | each split, MS-CLOSEOUT-02 | module doc comments and line counts | PENDING |
| SPLIT-7 history | each split | `git mv` renames; `git log --follow` works | PENDING |
| VERIFY-1 per split | each split | fmt, Clippy, tests, broker suites | PENDING |
| VERIFY-2 public items | MS-BASE-03, MS-NM-04, MS-TLS-04, MS-BLUEZ-03 | `all.html` comparisons | PENDING |
| VERIFY-3 exact candidate | MS-VALIDATE | full validation and CI | PENDING |
| VERIFY-4 review | MS-CLOSEOUT-01 | resolved review findings | PENDING |

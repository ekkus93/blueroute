# Module Split Specification

**Status:** Proposed; no split is implemented or accepted by this document.  
**Baseline:** `1870e406836174c0e0433f42d026f71e497b73f5` (2026-11-16).  
**Execution backlog:** [MODULE_SPLIT_TODO.md](MODULE_SPLIT_TODO.md).  
**Related:** [product specification](SPEC.md), [shell-free audit](P14-005-SHELL-FREE-AUDIT.md),
[original backlog](TODO.md).

## 1. Problem and scope

Four source files each combine several unrelated responsibilities in one module. Each is large
enough that a reviewer must read far more than the concern being changed, and in two of them the
security- or ownership-critical code sits among unrelated plumbing:

| File | Production lines | Inline tests | Responsibilities mixed today |
| --- | ---: | ---: | --- |
| `apps/cli/src/main.rs` | 1,167 | 22 | command tree and parsing; ~20 output formatters; local-key verification; error type; command dispatch |
| `crates/blueroute-linux/src/networkmanager.rs` | 1,787 | 14 | D-Bus proxies; inventory and change events; ownership metadata (what is safe to delete); owned profile lifecycle; addresses; routes |
| `apps/daemon/src/control_tls.rs` | 1,598 | 22 | identity persistence and key wiping; certificate parsing and pinning; enrollment certificate exchange; pinned TLS connect/accept and stream; authenticated session |
| `crates/blueroute-linux/src/bluez.rs` | 1,348 | 15 | adapter/peer discovery and signals; a complete pairing-agent subsystem (lines 165–815) |

Counts are from the baseline: production lines precede the file's `#[cfg(test)]` module; test
counts are harness listings (`cargo test -p <crate> --lib|--bins --locked <filter> -- --list`).

The work is a **behavior-preserving refactor**: each file becomes a directory module whose
submodules each own one responsibility. No feature, wire, storage, D-Bus, CLI output, security or
ownership behavior changes.

**Out of scope.** Files that are large but cohesive stay as they are: `single_star.rs`,
`join_network.rs` and `pan.rs` (one feature or subsystem each), and files whose size is mostly
tests (`control_worker.rs`, `removal.rs`, `control_listener.rs`). `apps/daemon/src/lib.rs` (the
D-Bus façade) and `crates/blueroute-client/src/lib.rs` are borderline; they may be assessed at
closeout but are not split by this plan.

## 2. Target layouts

Line ranges refer to the baseline and are guidance, not contracts; items may move between
neighbouring submodules when that keeps a dependency one-directional.

**CLI** (`apps/cli/src/`):

| Module | Contents (baseline lines) |
| --- | --- |
| `main.rs` | module declarations and `main` only |
| `args.rs` | `Cli`, the command enums, `Operation`, `parse`, `parse_node_id`, `parse_network_id` (16–371) |
| `format.rs` | `format_*`, `safe_diagnostic_field`, `control_peer_capacity`, `mutation_ack` (372–595, 716–835) |
| `local_key.rs` | `require_stable_local_identity`, `format_local_key`, `verify_local_key` (596–715) |
| `failure.rs` | `CliFailure` and its `From` impls (836–889) |
| `run.rs` | `run` dispatch (890–1162) |
| `error_guidance.rs` | unchanged |

**NetworkManager backend** (`crates/blueroute-linux/src/networkmanager/`):

| Module | Contents |
| --- | --- |
| `mod.rs` | constants, `NetworkManagerBackend` and its two trait impls as thin delegations |
| `dbus.rs` | availability check, proxies, settings accessors, `object_path`, `network_error` |
| `inventory.rs` | connection/device listing, snapshots, `NetworkManagerSubscription`, change queues |
| `ownership.rs` | `OwnedProfile`, profile matching, `exactly_one_owned_profile`, foreign-claim rejection, `profile_owner`, `profile_uuid`, `base_owned_settings`, `validate_interface_name` |
| `profile.rs` | bridge ensure/remove, add/update/activate/reapply/deactivate, device lookup, profile waits |
| `addresses.rs` | owned addresses, ensure/remove, address waits, prefix encode/decode |
| `routes.rs` | owned routes, ensure/remove, route waits, validation, route entry encode/decode |

**Control TLS** (`apps/daemon/src/control_tls/`):

| Module | Contents |
| --- | --- |
| `mod.rs` | shared constants, `ControlTlsError`, re-exports of the existing public items |
| `identity.rs` | `LocalControlTlsIdentity` (with its key-wiping `Drop`), `ControlTlsIdentityStore`, identity record encode/decode |
| `certificate.rs` | `PinnedControlTlsCertificate`, `ControlTlsCertificateCandidate`, `PinnedControlTlsPeer`, fingerprint/SPKI/node-name parsing |
| `enrollment.rs` | `exchange_control_tls_certificate`, `exchange_control_tls_candidate` and their bounded I/O helpers |
| `transport.rs` | `connect_pinned_control_tls`, `accept_pinned_control_tls`, peer verification, `ControlTlsStream` and its I/O impls |
| `session.rs` | `AuthenticatedControlPeer`, `ActiveControlSession`, `ControlSessionRead`/`Error`, `export_control_session_id` |

**BlueZ** (`crates/blueroute-linux/src/bluez/`): `mod.rs` keeps `BluezBackend`, its trait impl,
discovery, managed objects and signal subscriptions; `pairing.rs` takes the pairing control,
permit, agent, incoming pairing, pair and trust functions. `BluezBackend`'s `pub(crate)` fields
used by `pan.rs` keep their path and visibility.

## 3. Requirements

**SPLIT-1 — Move, don't change:** Code moves verbatim. The only permitted edits are module
declarations, `use` paths, visibility needed for the new boundaries, and module-level doc
comments. No logic, ordering, error text, constant value, log message or test assertion changes.
A change that turns out to be needed is a separate, reviewed commit with its own justification.

**SPLIT-2 — Public surface unchanged:** Every crate's public API stays identical, including the
`blueroute_linux` re-exports (`NetworkManagerBackend`, `BluezBackend`) and the daemon's
`pub use control_tls::{…}` list. Dependent crates compile without edits. The CLI's command tree,
help text, JSON and human output, error guidance and exit statuses are unchanged.

**SPLIT-3 — Minimal visibility:** New submodules are private. Items get the narrowest visibility
that compiles, preferring `pub(super)`. Nothing becomes `pub` that was not, and nothing is widened
to `pub(crate)` unless another existing module already needs it. In `control_tls`, private key
material, the identity record format and verifier internals stay private to their submodule.

**SPLIT-4 — Tests travel with their code:** Each inline test moves into the `tests` module of the
submodule it exercises. No test is deleted, renamed, merged, weakened or newly ignored. The
multiset of test function names per crate is identical before and after, and the harness counts
stay at the baseline: CLI 22, `blueroute-linux` 153 (14 under `networkmanager::`, 15 under
`bluez::`), `blueroute-daemon` 343 (22 under `control_tls::`). Only module paths in test names may
change.

**SPLIT-5 — Invariants preserved:** Shell-free production code, no `unsafe`, fail-closed
ownership metadata (foreign or malformed profiles are never adopted or deleted), pinned TLS
verification, key wiping on drop, bounded enrollment I/O, closed production join and disabled
sharing all stay as they are. `docs/SPEC.md` needs no change because no invariant changes; if one
would, the split stops and the change is planned separately.

**SPLIT-6 — One responsibility per module:** Every new module opens with a doc comment naming its
single responsibility. As a guide no new module exceeds about 600 production lines; a larger one
needs a written reason in the evidence.

**SPLIT-7 — Reviewable history:** One file is split per task, in small commits that each build
and pass. The original file is renamed with `git mv` to the new `mod.rs` (or its main successor)
so `git log --follow` keeps its history. Moved code is checked with
`git diff --color-moved=zebra --color-moved-ws=allow-indentation-change` (or an equivalent moved-
block check), and every non-moved line in the diff is accounted for.

## 4. Verification

**VERIFY-1 — Per split:** `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets
--locked -- -D warnings`; `cargo test --workspace --locked`; the split crate's test listing
compared with the baseline names (SPLIT-4); and for daemon/Linux splits the private-broker suites
in `ci.yml`. The CLI split also compares, byte for byte, the stdout, stderr and exit status of
every command in the tree (with and without `--json`), of invalid arguments and of help/version,
run with the system bus pointed at a socket that does not exist. (This CLI deliberately prints one
top-level help screen for every help request, so per-subcommand help adds nothing.)

**VERIFY-2 — Public surface:** Compare the item lists in `cargo doc --no-deps` output
(`target/doc/<crate>/all.html`) for `blueroute_linux` and `blueroute_daemon` before and after;
they must be identical. Confirm that dependent crates needed no source edits.

**VERIFY-3 — Exact candidate:** After all splits, run the full command reference in the TODO
(Rust, experimental-join, every broker suite, Python, frontend, packages, lifecycle and isolated
upgrade), record package hashes, and read exact-revision CI with `gh`.

**VERIFY-4 — Review:** An independent reviewer reads each split's diff for non-move changes,
visibility widening and lost tests, and specifically re-reads the moved ownership (NetworkManager)
and identity/verification (control TLS) code in its new location.

## 5. Evidence and closure

Record baseline and final listings, help-text and public-surface comparisons, per-module line
counts, commit SHAs, validation results and CI in `docs/MODULE_SPLIT_EVIDENCE.md`. Map every
requirement above to tasks and evidence. The refactor has software-only acceptance and needs no
equipment; it changes no product claim. Existing qualification gates stay open in their own
backlogs.

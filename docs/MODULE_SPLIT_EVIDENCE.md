# Module Split Evidence

Versioned evidence for [MODULE_SPLIT_TODO.md](MODULE_SPLIT_TODO.md) (specification:
[MODULE_SPLIT_SPEC.md](MODULE_SPLIT_SPEC.md)). Software-only refactor; no product claim changes.

## MS-BASE — Baseline (2026-11-16)

**Environment (MS-BASE-01).** Source `48f5dda` (`1870e40` plus the two plan documents; no code
difference), clean tracked tree apart from `.serena/` and `AGENTS.md`. rustc/cargo 1.95.0, Debian
13 kernel 6.12.115.

**Files to split** (production lines precede the file's `#[cfg(test)]` module):

| File | Total lines | Production | Tests in file |
| --- | ---: | ---: | ---: |
| `apps/cli/src/main.rs` | 2,090 | 1,167 | 22 |
| `crates/blueroute-linux/src/networkmanager.rs` | 2,107 | 1,787 | 14 |
| `apps/daemon/src/control_tls.rs` | 2,907 | 1,598 | 22 |
| `crates/blueroute-linux/src/bluez.rs` | 1,643 | 1,348 | 15 |

**Baseline snapshots (MS-BASE-02/03)**, taken twice with identical results:

| Snapshot | Baseline |
| --- | --- |
| `cargo test -p blueroute --bins -- --list` | 22 tests |
| `cargo test -p blueroute-linux --lib -- --list` | 153 tests (14 `networkmanager::`, 15 `bluez::`) |
| `cargo test -p blueroute-daemon --lib -- --list` | 343 tests (22 `control_tls::`) |
| CLI behavior matrix | 62 cases (31 argv, each with and without `--json`): exit 0 ×5, 2 ×20, 3 ×37 |
| `blueroute_linux` public doc items (`all.html` links) | 81 |
| `blueroute_daemon` public doc items | 155 |

The CLI prints one top-level help screen for every help request (`parse` maps clap's
`DisplayHelp` to `Operation::Help`), so the matrix covers help through `--help` and `help network`
rather than per-subcommand help. With the system bus pointed at a missing socket, commands that
need the daemon fail with exit 3 and the daemon-unavailable guidance before sending anything.

**Comparison method (MS-BASE-04).** The capture script below is run as `capture.sh base` before
and `capture.sh <task>` after each split; a split passes SPLIT-2/4 when the leaf test-name
multisets (`leaf-*`), the CLI case files (`cli/`) and the doc item lists (`doc-*`) are identical
to the baseline. Full test paths may differ only by module path. The script (kept here so the
comparison does not depend on ignored files):

```bash
#!/usr/bin/env bash
# MS capture: test-name listings, CLI help text and public doc items, into tmp/ms/<label>/.
set -uo pipefail
repo=/home/phil/work/blueroute; cd "$repo" || exit 1
export TMPDIR=$repo/tmp
out=$repo/tmp/ms/${1:?label}; rm -rf "$out"; mkdir -p "$out"
git rev-parse HEAD > "$out/head"; git status --short > "$out/status"
list() { cargo test --locked "$@" -- --list 2>/dev/null | sed -n 's/: test$//p' | sort; }
list -p blueroute --bins > "$out/tests-cli"
list -p blueroute-linux --lib > "$out/tests-linux"
list -p blueroute-daemon --lib > "$out/tests-daemon"
leaf() { sed 's/.*:://' "$1" | sort; }
for s in cli linux daemon; do leaf "$out/tests-$s" > "$out/leaf-$s"; done
cargo build --locked -p blueroute -q 2>/dev/null
bin=$repo/target/debug/blueroute
# CLI behavior with no reachable daemon: the system bus address names a socket that does not
# exist, so every command fails before anything is sent. stdout, stderr and exit status per argv.
node=00112233445566778899aabbccddeeff
fp=0000000000000000000000000000000000000000000000000000000000000000
mkdir -p "$out/cli"
i=0
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    i=$((i+1)); read -ra argv <<< "$line"
    for json in "" "--json"; do
        f=$(printf '%s/cli/%03d%s' "$out" "$i" "${json:+-json}")
        { echo "argv: $line $json"
          DBUS_SYSTEM_BUS_ADDRESS="unix:path=$repo/tmp/ms/no-such-bus" "$bin" "${argv[@]}" $json \
              > "$f.out" 2> "$f.err"; echo "exit: $?"; } > "$f.meta"
    done
done <<LIST
--help
--version
help network
status
capability show
diagnostics show
diagnostics show --summary
network list
network create Home
network create
network join $node
network leave
network link
network reconnect
node list
node show $node
node show not-a-node
node set-name Laptop
peer local-key
peer verify-local-key $node $fp
peer verify-local-key $node $fp --network $node
peer verify-local-key $node short
peer trust $node
peer enroll $node $fp
peer forget $node
peer set-join-host $node $node $fp
peer clear-join-host
discover start
discover stop
no-such-command
network
LIST
cargo doc --no-deps --locked -q -p blueroute-linux -p blueroute-daemon 2>/dev/null
for c in blueroute_linux blueroute_daemon; do
    grep -o 'href="[^"]*"' "$repo/target/doc/$c/all.html" | sort > "$out/doc-$c"
done
{ for s in cli linux daemon; do echo "$s $(wc -l < "$out/tests-$s")"; done
  echo "networkmanager $(grep -c '^networkmanager::' "$out/tests-linux")"
  echo "bluez $(grep -c '^bluez::' "$out/tests-linux")"
  echo "control_tls $(grep -c '^control_tls::' "$out/tests-daemon")"
  echo "cli-cases $(ls "$out/cli"/*.meta | wc -l)"
  for c in blueroute_linux blueroute_daemon; do echo "doc-$c $(wc -l < "$out/doc-$c")"; done; } | tee "$out/summary"
```

## MS-CLI — CLI entry point split (2026-11-16)

`apps/cli/src/main.rs` (2,090 lines) is now `main.rs` (module declarations and `main`, 13 lines)
plus, with production / total lines:

| Module | Responsibility | Production | Total |
| --- | --- | ---: | ---: |
| `args.rs` | command tree and parsing into an `Operation` | 364 | 702 |
| `format.rs` | human and JSON rendering, escaping untrusted daemon text | 357 | 724 |
| `local_key.rs` | reading and independently verifying the local control-TLS key | 127 | 333 |
| `failure.rs` | `CliFailure`: stable exit codes and error text | 62 | 87 |
| `run.rs` | running one parsed command and printing its result | 286 | 286 |
| `error_guidance.rs` | unchanged | 72 | 131 |

**Move-only check (SPLIT-1).** A line-multiset comparison of the original file with the new files
(ignoring module docs, `use` and `mod` declarations, blank lines and `pub(super)` prefixes) leaves
only: the three test-module headers and their closing braces; `run(&args)` becoming
`run::run(&args)` in `main`; and three formatter signatures (`format_status`,
`format_capabilities`, `format_networks`) that rustfmt rewrapped because `pub(super)` pushed them
past 100 columns. No logic line differs.

**Visibility (SPLIT-3).** Only items used by a sibling became `pub(super)`: `Cli`, `Operation`,
`parse`, the nine formatters `run` calls, `mutation_ack`, the three local-key functions,
`CliFailure` and its three fields (read by `run`), and `run`. Nothing is `pub`.

**Tests (SPLIT-4).** 22 tests, the same leaf names as the baseline: `args` 4 (with the `args`
test helper), `format` 12, `local_key` 2, `failure` 1, `error_guidance` 3. The two tests that
check both parsing and output (`diagnostic_summary_is_opt_in…`, `explicit_mutation_commands…`)
sit in `args` and import the formatter they check.

**Verification (VERIFY-1).** `capture.sh cli` against the baseline: leaf test names identical for
all three crates; all 62 CLI behavior cases (stdout, stderr, exit status) byte-identical; both
crates' public doc items identical. fmt clean; workspace Clippy clean; workspace tests 933 passed,
0 failed, 97 ignored (unchanged); `cli_exit_contract` broker test passed. The CLI keeps its file
name, so history needs no rename; `git blame -C` follows the moved lines.

## MS-NM — NetworkManager backend split (2026-11-16)

Two commits. `4f8df40` moves `networkmanager.rs` to `networkmanager/mod.rs` with identical content
(git records the rename, so `git log --follow` keeps the history); the next commit splits it:

| Module | Responsibility | Production | Total |
| --- | --- | ---: | ---: |
| `mod.rs` | constants, `NmSettings`, `NetworkManagerBackend` and its two trait impls; test-only `test_support` | 236 | 249 |
| `dbus.rs` | availability, typed proxies, settings-map accessors, error mapping | 167 | 167 |
| `inventory.rs` | connection/device listing, snapshots, `NetworkManagerSubscription`, change events | 291 | 334 |
| `ownership.rs` | ownership metadata: owner parsing, profile matching, exactly-one-owned, foreign-claim rejection, profile UUID, interface-name validation | 298 | 425 |
| `profile.rs` | owned profile lifecycle: bridge ensure/remove, add, update, activate, reapply, deactivate, waits | 309 | 309 |
| `addresses.rs` | owned addresses and the profile address encoding | 279 | 328 |
| `routes.rs` | owned routes, validation and the profile route encoding | 373 | 498 |

The split was generated by a move-only splitter (each top-level item copied verbatim, with its
docs and attributes, in original order; every item and test assigned exactly once by name; unused
imports then removed by `cargo fix`). The ownership rules (SPLIT-5) are all in `ownership.rs` and
nothing outside `networkmanager` can reach them: the crate still exports only
`NetworkManagerBackend`.

**Move-only check (SPLIT-1).** The line-multiset comparison leaves only scaffolding (four new test
modules and the `test_support` module, with their `#[cfg(test)]` lines and braces) and seven
signatures rustfmt rewrapped after `pub(super)` was added (`ensure_networkmanager_available`,
`list_connections`, `get_settings`, `owned_addresses`, `ensure_route`, `remove_route`,
`set_address_prefixes`).

**Visibility (SPLIT-3).** Constants and `NmSettings` stay private in `mod.rs` (children see their
parent's private items). Forty functions used across submodules are `pub(super)`, as are
`OwnedProfile` and `NetworkManagerSubscription` and their fields (read by the profile, address
and route modules and constructed in `mod.rs`). Nothing is `pub` that was not.

**Tests (SPLIT-4).** 14 tests, same leaf names: `ownership` 7, `routes` 4, `addresses` 2,
`inventory` 1. The two shared helpers `network` and `interface` moved once to `mod.rs`'s
`#[cfg(test)] mod test_support` rather than being copied.

**Verification (VERIFY-1/2).** `capture.sh nm`: leaf test names identical for all three crates
(`blueroute-linux` 153, 14 under `networkmanager::`); `blueroute_linux` and `blueroute_daemon`
public doc items identical; CLI matrix identical. fmt clean; workspace Clippy clean; workspace
tests 933 passed, 0 failed, 97 ignored; all 17 private-broker suites passed (96 tests).

## MS-TLS — Control TLS split (2026-11-16)

Two commits: `13e4469` moves `control_tls.rs` to `control_tls/mod.rs` unchanged (recorded as a
rename); the next commit splits it with the same move-only splitter.

| Module | Responsibility | Production | Total |
| --- | --- | ---: | ---: |
| `mod.rs` | module doc, constants, `ControlTlsError`, zero-ID checks, re-exports; test-only `test_support` | 111 | 210 |
| `identity.rs` | local identity, its persisted record format and storage, key wiping on drop | 313 | 600 |
| `certificate.rs` | pinned certificates and peers, enrollment candidates, strict SPKI/node-name parsing | 232 | 262 |
| `enrollment.rs` | the bounded certificate-preface exchange (unauthenticated candidates only) | 240 | 354 |
| `transport.rs` | pinned mutual TLS 1.3 connect/accept, peer verification, the TLS stream | 453 | 751 |
| `session.rs` | the authenticated control session: exporter-bound Hello, framed I/O, session errors | 348 | 913 |

**Public surface (SPLIT-2).** `mod.rs` re-exports every item that was `pub` in the original file
(`pub use self::<module>::{…}`), so `crate::control_tls::X` paths and the daemon's own
`pub use control_tls::{…}` list are unchanged; those items keep their original `pub`. The
`blueroute_daemon` public doc items are identical to the baseline.

**Visibility (SPLIT-3).** `pub(super)` only where a sibling needs it:

- functions: `certificate_fingerprint`, `certificate_names_node`, `certificate_node_id`,
  `node_dns_name`, `verify_control_tls_peer_identity`, and the method
  `ControlTlsStream::send_close_notify_until`;
- fields read across modules: `node`/`network`/`fingerprint` of the pinned and verified peer
  types, `PinnedControlTlsCertificate.certificate`, all fields of
  `ControlTlsCertificateCandidate` (built in `enrollment`) and of `ActiveControlSession` (built
  in `transport`), `AuthenticatedControlPeer`'s `peer`/`session_id`/`facts`,
  `LocalControlTlsIdentity`'s `node` and `certificate`, and `ControlTlsStream`'s `socket` and
  `connection`.

**Key material stays private.** `LocalControlTlsIdentity.private_key` is still private to
`identity.rs`. Three tests read it directly (as the single-module file allowed), so they live in
`identity`'s test module rather than with the certificate/enrollment code they exercise:
`certificate_parsers_reject_valid_der_with_trailing_untrusted_bytes`,
`certificate_preface_is_bounded_and_matches_only_the_approved_pin` and
`enrollment_candidate_requires_independent_fingerprint_confirmation`. The last one also reads
`ControlTlsStream.connection`; with no existing accessor, that field is `pub(super)` (visible only
inside `control_tls`) instead of widening `private_key`. The rustls connection holds session state,
not the identity's private key. `preface_factories_establish_only_pinned_tls13_connections` reads
only `connection` and sits in `transport`.

**Tests (SPLIT-4).** 22 tests, same leaf names: `identity` 6, `transport` 6, `session` 6,
`enrollment` 3, `certificate` 1. The shared helpers (`TempDirectory`, `NEXT_TEMP`, `node`,
`network`, `make_tls_pair`, `stream_pair`, `hello_facts`, `trust_for_peer`, `active_deadline`)
moved once to `test_support`.

**Move-only check (SPLIT-1).** Only scaffolding (five test modules / `test_support` with their
braces) and `certificate_node_id`'s signature rewrapped by rustfmt, plus `TempDirectory`'s field
visibility. Some imports that only tests had used through `use super::*` were added back into the
test modules that need them (`Duration`, `Instant`, `Arc`, `SpkiSha256`, `HelloFacts`,
`ConnectionDeadline`, `CertificateParams`, `KeyPair`, `Digest`, `FromDer`, `Write`).

**Verification (VERIFY-1/2).** `capture.sh tls`: leaf test names identical (daemon 343, 22 under
`control_tls::`); public doc items and CLI matrix identical. fmt clean; workspace Clippy clean,
daemon Clippy clean with `experimental-join`; workspace tests 933 passed, 97 ignored;
`experimental-join` 392 passed, 16 ignored; all 17 private-broker suites passed (96 tests).
Production lines per module are at most 453.

## MS-BLUEZ — Pairing agent moved out of the BlueZ backend (2026-11-16)

Two commits: `ecf9c7f` moves `bluez.rs` to `bluez/mod.rs` unchanged (recorded as a rename); the
next commit moves the pairing subsystem to `bluez/pairing.rs`.

| Module | Responsibility | Production | Total |
| --- | --- | ---: | ---: |
| `mod.rs` | constants, `BluezBackend` and its `BluetoothBackend` impl, managed objects, adapter/peer discovery and their signal subscriptions | 725 | 935 |
| `pairing.rs` | pairing control (one operation at a time), the no-input/no-output agent, incoming pairing windows, outgoing pairing and trust | 669 | 759 |

**Size (SPLIT-6).** Both modules exceed the ~600-line guide. The plan for this file was only to
separate pairing; what remains in `mod.rs` is one subsystem (BlueZ object enumeration, discovery
and the signals that keep those snapshots current), and `pairing.rs` is the single pairing agent
and its control. Splitting either further would cut through tightly coupled code for little
review benefit, so they stay as they are.

**Move-only check (SPLIT-1).** The line-multiset comparison leaves only the new test module
header and its closing brace: no signature needed rewrapping.

**Visibility (SPLIT-3).** `pub(super)`: `PairingControl` and `PairingAgent` (field types of
`BluezBackend`) and the four operations `BluezBackend` calls (`begin_incoming_pairing`,
`end_incoming_pairing`, `pair_peer`, `set_peer_trusted`). Constants stay private in `mod.rs`.
`BluezBackend`'s `pub(crate)` fields and the `crate::bluez::BluezBackend` path that `pan.rs` uses
are unchanged; no field needed widening.

**Tests (SPLIT-4).** 15 tests, same leaf names: the six pairing tests moved to `pairing`; the nine
discovery tests and their three helpers stay in `mod.rs`.

**Verification (VERIFY-1/2).** `capture.sh bluez`: leaf test names identical (`blueroute-linux`
153, 15 under `bluez::`); public doc items and CLI matrix identical. fmt clean; workspace Clippy
clean; workspace tests 933 passed, 97 ignored. Private-broker suites are run in MS-VALIDATE.

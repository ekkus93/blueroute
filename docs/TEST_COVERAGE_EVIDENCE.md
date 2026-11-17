# Test Coverage Evidence

**Backlog:** [TEST_COVERAGE_TODO.md](TEST_COVERAGE_TODO.md).
**Specification:** [TEST_COVERAGE_SPEC.md](TEST_COVERAGE_SPEC.md).

## TC-BASE — Baseline (2026-11-16)

**Revision.** HEAD `83d9752d4e9f4d61330f4e25a38d23d1cb198932` (the specification baseline
`e00aa28` plus two documentation-only commits), clean apart from the never-committed `.serena/`
and `AGENTS.md`. Toolchain `rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0`.

**Gap table.** Re-read against the code at this revision: the function names, files and test
counts in the specification's gap table are accurate. Per-module unit-test counts:
`networkmanager::routes` 4, `networkmanager::addresses` 2, `kernel_address` 3,
`bluez::pairing` 6; `networkmanager::profile` and `networkmanager::inventory` have no tests of
their own. No test drives `NetworkManagerBackend` over D-Bus.

**Listings.** Sorted `name: test` lines from the harness, compared by multiset:

| Command | Tests |
| --- | ---: |
| `cargo test -p blueroute-linux --lib --locked -- --list` | 153 |
| `cargo test -p blueroute-linux --tests --locked -- --list` | 172 |
| `cargo test -p blueroute --locked -- --list` | 29 |

The `blueroute-linux --tests` listing is the library's 153 plus 19 integration tests:
`network_backend_contract` 4, `route_projection_permutations` 6 and `route_projection_contract`
9, the last being the library's own `route_projection::tests` compiled a second time. The CLI
listing is the binary's 22 unit tests plus `front_end_boundary` 2 and `cli_exit_contract` 5.

**How comparisons are repeated.** Re-run the three commands, keep lines ending in `: test`, sort
them, and diff against the previous listing. A task may only add names, except TC-DEDUP, which
removes exactly the nine duplicate `route_projection::tests::*` names from the `--tests` listing.

## TC-PARSE — Parser rejection tables (2026-11-16)

Eleven unit tests added in three files; no production line changed.

| File | Tests added | Covers |
| --- | ---: | --- |
| `networkmanager/routes.rs` | 3 | `route_key_from_entry`: 17 rejection cases (unsupported key; missing, non-string, invalid and wrong-family `dest`; missing, string, 256, IPv4 33 and IPv6 129 `prefix`; invalid, wrong-family both ways and non-string `next-hop`; missing and string `metric`) each with its `ErrorKind`; accepted boundaries (/32, /128, empty and same-family `next-hop`); `route_data_entries` absent group or key versus malformed container |
| `networkmanager/addresses.rs` | 2 | `address_prefixes`: missing or non-string address, missing or string prefix, invalid address, prefix 256, IPv4 33, IPv6 129 and a malformed `address-data` container, each with its kind; absent group or key, /32 and /128 accepted, duplicates deduplicated |
| `kernel_address.rs` | 6 | `netlink_message` (15 bytes, declared length 0, 15 or past the buffer, exactly 16, payload bounded by the declared length); `parse_netlink_ack` (empty and 3-byte, 0 with trailing bytes, `-EEXIST` → `InvalidState`, `-EPERM`, `-ENODEV`, positive and `i32::MIN` → `NetworkBackendUnavailable` with the exact `errno=` diagnostic); `parse_ipv4_address` (message under 8 bytes, truncated attribute header, attribute length 0, 3 or past the end; INET6, other interface, no attributes, empty and wrong-width values skipped; `IFA_ADDRESS` fallback; `IFA_LOCAL` preferred in either order; unaligned unknown attribute followed by a valid one; flag bits above `NLA_TYPE_MASK`; unpadded final attribute) |

**Listing.** `blueroute-linux --lib` grew from 153 to 164: exactly the eleven new names, none
removed.

**Negative controls** (scratch worktree `tmp/tc/neg` holding the new tests, each mutation applied
by a helper that aborts with `CONTROL NOT APPLIED` unless its anchor matches exactly once, then
restored):

| Control | Mutation | Failing test |
| --- | --- | --- |
| A | a missing route `metric` becomes metric 0 | `route_data_rejection_table_fails_closed_with_typed_errors` |
| B | `netlink_message` drops the `length > bytes.len()` check | `netlink_message_rejects_truncated_and_inconsistent_headers` |
| C | `IFA_ADDRESS` takes precedence over `IFA_LOCAL` | `address_parser_follows_aligned_attributes_and_prefers_local` |
| D | an address prefix is truncated with `& 0xff` instead of rejected | `address_data_rejection_table_fails_closed_with_typed_errors` |

Each control failed exactly the named test and nothing else in its module.

**Findings (COV-2).** None. One observation, not a defect: `address_prefixes` does not check that
an address sits under the matching family group (an IPv6 address under `ipv4` is read as
valid). BlueRoute writes `address-data` only through `set_address_prefixes`, which files each
prefix under its own family. It is not asserted either way and is left for TC-CLOSEOUT review to
decide whether it warrants a separate task.

**Checks.** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D
warnings` and `cargo test -p blueroute-linux --locked` (164 + 4 + 9 + 6) pass. The over-width
lines reported in `routes.rs` and `addresses.rs` are pre-existing production error strings that
rustfmt leaves alone; none are in the new tests.

## TC-PAIR — Pairing error mapping and agent refusal (2026-11-16)

Six unit tests added to `crates/blueroute-linux/src/bluez/pairing.rs`; no production line
changed.

| Test | Covers |
| --- | --- |
| `every_pairing_method_error_name_maps_to_its_kind_and_message` | all nine named arms of `pairing_method_error` plus two unknown names (default arm), asserted both directly and through `pairing_error`, with kind, message and diagnostic |
| `pairing_transport_errors_map_timeouts_apart_from_other_failures` | `pairing_error`: `NoReply`, `TimedOut` and `Timeout` → timed out; `fdo::Failed` and a transport failure → failed |
| `trust_errors_map_missing_peers_apart_from_refusals` | `trust_error` (every guarded name, an unknown name, a transport failure) and `trust_property_error` (`UnknownObject`, `AccessDenied`, `AuthFailed`, `Failed`) |
| `incoming_pairing_and_agent_registration_errors_map_to_typed_kinds` | `incoming_pairing_error` (every guarded name → `MissingAdapter` or `AuthenticationFailed`, others → `CapabilityUnavailable`), `incoming_pairing_property_error` (all four arms) and `pairing_agent_registration_error` (kind, message and diagnostic) |
| `every_agent_callback_refuses_devices_outside_an_active_permit` | all five device-taking callbacks refuse with the authorization message when no operation is active, for another peer, a device on another adapter and a child object of the permitted peer, and after the permit is dropped; for the permitted peer, the three input callbacks still refuse with their own NoInputNoOutput messages and `request_authorization`/`authorize_service` accept |
| `every_agent_callback_refuses_devices_outside_the_incoming_window` | in an incoming window, devices on the window's adapter are accepted as above; a device on another adapter, a child object, the adapter path itself, a non-device child and an adapter-name prefix (`hci00`) are refused; everything is refused after the window ends |

**Listing.** `blueroute-linux --lib` grew from 164 to 170: exactly the six new names, none
removed.

**Negative controls** (same scratch worktree and helper as TC-PARSE):

| Control | Mutation | Failing tests |
| --- | --- | --- |
| E | `AuthenticationRejected` maps to `PairingFailed` | `every_pairing_method_error_name_maps_to_its_kind_and_message`, and the pre-existing `pairing_rejection_and_timeout_have_distinct_typed_errors` |
| F | `authorize_service` returns `Ok` without `require_authorized` | both agent-callback refusal tests; before this task no test called `authorize_service` |

**Findings (COV-2).** None.

**Checks.** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D
warnings` and `cargo test -p blueroute-linux --lib --locked` (170) pass.

## TC-CLI — Checked-in CLI behavior matrix (2026-11-16)

`apps/cli/tests/behavior_matrix.rs` adds one test,
`every_cli_command_matches_its_recorded_behavior_without_a_daemon`. It runs the built binary
(`CARGO_BIN_EXE_blueroute`) for the 31 command lines of the module-split capture, each with and
without `--json`, with `DBUS_SYSTEM_BUS_ADDRESS` naming a socket under `CARGO_TARGET_TMPDIR`
that never exists (the test asserts it is absent). Each case's argv, exit status, stdout and
stderr are compared byte for byte with `apps/cli/tests/behavior/NN-words[-json].txt`. The bus
address appears in connection diagnostics, so it is replaced with `{bus}` in the recorded and
compared text. All mismatches are reported together with the case file and the differing lines;
a recorded file that no case produces is also a failure. Files are rewritten only with
`BLUEROUTE_BEHAVIOR_BLESS=1`.

**Expected files.** Generated once from the current binary (CLI sources unchanged since
`a8f3fad`): 62 files, outcome mix 5 × exit 0, 20 × exit 2, 37 × exit 3. Each file was rebuilt
independently from the module-split capture `tmp/ms/final/cli/NNN[-json].{meta,out,err}` (the
capture's own bus path replaced with `{bus}`): **0 of 62 differ**.

The help screens end several lines with spaces (clap's column padding), so the recorded help
files intentionally contain trailing whitespace and `git diff --check` reports them; do not strip
it, or the matrix fails.

**Determinism.** Two consecutive non-blessing runs pass; the test needs no private bus and runs in
plain `cargo test --workspace`.

**Listing.** `cargo test -p blueroute --locked -- --list` grew from 29 to 30: the one new name.

**Negative controls** (same scratch worktree and helper):

| Control | Mutation | Result |
| --- | --- | --- |
| G | top-level `about` text "Manage BlueRoute networks" → "…meshes" | 3 of 62 cases differ (`01-help`, `01-help-json`, `03-help-network`), each showing the one changed line |
| H | JSON error key `"ok"` → `"success"` in `error_guidance.rs` | 29 of 62 cases differ: every `--json` case that fails (all except help and version) |

**Findings (COV-2).** None.

**Checks.** `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D
warnings` pass.

## TC-DEDUP — Duplicate route projection harness removed (2026-11-16)

`crates/blueroute-linux/tests/route_projection_contract.rs` contained only a `#[path =
"../src/route_projection.rs"] mod route_projection;` and an import, under a doc comment that still
called the module's wiring pending. Its harness listing (9 tests) was identical, name for name, to
the library's `route_projection::tests::*` listing from `--lib`. Nothing in the repository, CI or
other docs referenced the file outside this backlog. It was deleted.

| Listing | Before (baseline / TC-PAIR) | After |
| --- | ---: | ---: |
| `blueroute-linux --lib` | 170 | 170, identical names |
| `blueroute-linux --tests` | 172 at the baseline, 189 with TC-PARSE and TC-PAIR | 180 |
| `route_projection::tests::*` occurrences in `--tests` | 18 | 9 |

Against the baseline `--tests` multiset, the only names removed are one copy each of the nine
`route_projection::tests::*` names. `cargo test -p blueroute-linux --locked` (170 + 4 + 6) and
`cargo clippy -p blueroute-linux --all-targets --locked -- -D warnings` pass.

## TC-NM — Fake NetworkManager broker suite (2026-11-16)

**Files.** `crates/blueroute-linux/tests/networkmanager_backend/fake.rs` (the fake) and
`crates/blueroute-linux/tests/networkmanager_backend.rs` (one outer test and nine scenarios). No
production line changed and no dependency was added: the fake uses `zbus`, which the crate
already depends on, and integration tests may use the crate's normal dependencies.

**Fake (COV-3).** zbus `#[interface]` objects implement exactly the corrected COV-3 surface (see
the dated correction in the specification: the device's `State` property was missing from the
original list and `State` was wrongly listed on the active connection). Profiles, devices and
active connections live in fixed pools of object paths registered at start (32, 16 and 64); an
empty slot answers `UnknownObject`. Changes apply immediately, so every backend wait succeeds on
its first check (the waits poll before sleeping). Every mutating call is logged with its target
profile, device and flags, failed calls included, and any method can be scripted to fail with
`org.freedesktop.DBus.Error.Failed`. Activating a bridge profile without a device creates the
bridge device, as NetworkManager does for virtual interfaces.

**Execution.** The outer `#[ignore]` test re-runs its own binary with `DBUS_SYSTEM_BUS_ADDRESS`
set to the `dbus-run-session` bus and runs the `scenario_*` tests with `--test-threads=1`
(each owns `org.freedesktop.NetworkManager`); it requires the child to report exactly 9 passed.
The scenarios use the unmodified `NetworkManagerBackend::connect_system()`. Run directly with the
child variable set, the nine scenarios take 1.49 s; without it they return immediately (0.00 s),
as in the existing `watch_integration` pattern.

| Scenario | COV-4 item | Asserts |
| --- | --- | --- |
| `owned_bridge_lifecycle_is_idempotent_and_leaves_others_untouched` | 1, 2, 5 | `Version` and `service_owner`; `ensure_bridge` → exactly `AddConnection2` (flags `0x21`) and a device-less `ActivateConnection`, with the `user.data` ownership triple and `autoconnect=false` stored; repeat → no call; address and route ensure → `Update2` (flag `0x1`) + `Reapply`, repeat → `Reapply` only; removals → `Update2` + `Reapply`, repeats → no call; `addresses`, `routes`, `network_connections` and `network_devices` match the fake exactly; `remove_owned_interface` → `DeactivateConnection` + `Delete`, repeat → no call; foreign and other-network profiles byte-identical and the foreign device still active |
| `owned_interface_profile_is_created_only_on_a_known_free_device` | 1, 2 | unknown interface → `InvalidInput`, no call; known free device → `AddConnection2`, `Update2`, `ActivateConnection` on that device, `Reapply`, `generic` type and `interface` kind stored; removal deactivates and deletes only that profile |
| `removal_is_scoped_to_the_requested_network` | 2 | with A's active and B's inactive profile on one interface, removing B's deletes exactly B's profile; A's stays byte-identical and active |
| `foreign_and_other_network_claims_fail_closed` | 3 | six refusals (foreign claim, foreign active interface for bridge and address, other-network claim, other-network active interface, route without an owned profile), each checked for no mutating call **before** its `InvalidState` kind; removals of this network on every interface are no-ops with no call |
| `malformed_ownership_metadata_fails_every_operation_closed` | 3 | six variants (schema `2`, schema missing, kind missing, kind `tunnel`, invalid owner → `InvalidState`; `user.data` not a string map → `ProtocolError`) on an unrelated interface: all six mutations on both the owned and the malformed interface, and `addresses`, `routes`, `network_connections`, fail with that kind and no call; the owned profile stays active |
| `duplicate_owned_profiles_fail_closed` | 3 | two profiles of one network on one interface: all six mutations `InvalidState` with no call; read-only listings still succeed |
| `absent_networkmanager_is_backend_unavailable` | 4 | `connect_system` before the fake owns the name; after it releases the name, `service_owner` is `None` and `version`, the four listings and all six mutations are `NetworkBackendUnavailable` |
| `scripted_method_failures_are_backend_unavailable_and_stop_the_operation` | 4 | each of `AddConnection2`, `ActivateConnection`, `Update2`, `Reapply`, `DeactivateConnection`, `Delete`, `ListConnections`, `GetSettings` and `GetDevices` failing → `NetworkBackendUnavailable`; a failed mutating call is the last call made; no call targets the foreign or other-network profile, which stay byte-identical |
| `state_subscription_reports_inventory_then_changes` | 5 | subscription yields the seeded connection and device, then the profile added afterwards |

**Scheduling (COV-9).** A CI step "Run the NetworkManager backend against a fake NetworkManager
(TC-NM)" runs `dbus-run-session -- cargo test -p blueroute-linux --test networkmanager_backend
--locked -- --ignored`, and `CLAUDE.md` lists the suite with the other private-bus suites.

**Repetition.** Three consecutive local runs: `10 passed` each (about 1 s).

**Negative controls** (scratch worktree with the new suite; each control rebuilt and the child
run directly):

| Control | Mutation | Reported failure |
| --- | --- | --- |
| I | `remove_owned_interface` takes any profile on the interface and skips its owner re-check (no ownership check before `Delete`) | "removals on eth0 touched NetworkManager": `DeactivateConnection` and `Delete` on the foreign `Wired` profile; the duplicate scenario also fails (`remove_owned_interface` succeeded) |
| J | `matching_profiles` keeps every profile and `ensure_address` skips its owner re-check (no ownership check before `Update2`) | "address on a foreign active interface: touched NetworkManager": `Update2` and `Reapply` on `Wired`; the scoped-removal scenario also fails |
| K | `ensure_bridge` skips `reject_foreign_interface_claim` | "bridge on an interface a foreign profile claims: touched NetworkManager": `AddConnection2` and `ActivateConnection` |
| L | `ensure_address` skips `reject_foreign_active_connection` | "address on a foreign active interface: touched NetworkManager": `AddConnection2` and `Update2` |

A first run of I and J removed both the shared owner filter and the re-check; the suite failed
then too, but at an earlier assertion (an adopted other-network bridge, an ambiguity error), so
the refusal checks were changed to inspect the call log per case before the error kind, and the
removal loop to use a removal-only helper. The controls above are from the final suite and show
the foreign-path call-log assertions are reached.

**Findings (COV-2).** None. Asserted as designed: a single malformed BlueRoute profile anywhere
makes every operation and listing fail closed, not just operations on its interface.

**Checks.** `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --locked -- -D
warnings` pass.

## TC-VALIDATE — Exact-candidate validation (2026-11-16)

Validation ran twice: on the pre-review candidate `d7640c0` and, after the review fixes below, on
the final code candidate `36de5c85abb9f60bd80980536db0aa07298e19fa`. Both runs used the same
script, the TODO's checks with the new suite added: `cargo fmt --check`, `cargo build`, `cargo
check`, workspace clippy with `-D warnings`, workspace tests, experimental-join tests and
clippy, the removal tests, every private-bus suite in `ci.yml`, the Python tests, the desktop
frontend checks, and the closed-join gate tests. Toolchain: Rust 1.95.0, Node 26.13.0,
npm 11.20.0, Python 3.13.5, dbus-daemon 1.16.2.

| Check | `d7640c0` | `36de5c8` (final) |
| --- | --- | --- |
| fmt, build, check, clippy (workspace and experimental-join) | pass | pass |
| `cargo test --workspace --locked` | 942 passed, 107 ignored | 943 passed, 108 ignored |
| experimental-join | 392 passed | 392 passed |
| removal | 33 passed | 33 passed |
| private-bus suites | 18 of 18 pass; `networkmanager_backend` 10 | 18 of 18 pass; `networkmanager_backend` 11 |
| Python | 7 OK | 7 OK |
| frontend (`ci`, format, lint, typecheck, test, build) | all rc 0, 43 tests | all rc 0, 43 tests |
| closed join gate | pass | pass |

The workspace delta from the module-split result (933 passed, 97 ignored) is exact: +18
library tests (11 TC-PARSE, 6 TC-PAIR, 1 review fix), +1 CLI matrix, −9 duplicate
route-projection tests, and +11 ignored suite tests (one outer test, ten scenarios).

**No production change (TC-VALIDATE-02).** `git diff e00aa28..36de5c8` touches `.github/workflows/ci.yml`,
`CLAUDE.md`, docs, new files under `tests/`, and four `src` files only below their existing
`#[cfg(test)]` line (first changed lines 552, 498, 328 and 759; the test modules start at 511,
375, 281 and 671). The only removed code lines anywhere are the seven lines of the deleted
duplicate harness. No `Cargo.toml` or `Cargo.lock` change.

**CI (TC-VALIDATE-03).** Exact-revision runs, each read with `gh`, all success:

| Head | Content | Run |
| --- | --- | --- |
| `3b71fb3` | TC-PARSE | 44036478997 |
| `5e604b4` | TC-PAIR, TC-CLI, TC-DEDUP | 44037732550 |
| `d7640c0` | TC-NM | 44038994230 |
| `36de5c8` | review fixes (final code) | 44040584052 |

The `36de5c8` log shows the new CI step running the suite (`test result: ok. 11 passed`) and the
behavior matrix in the workspace tests.

## TC-CLOSEOUT — Review and close (2026-11-16)

**Independent review (TC-CLOSEOUT-01).** A separate reviewer read `e00aa28..d7640c0` against the
specification without the evidence, ran the linux, CLI and NetworkManager suites, and ran its
own mutations. It found no production change, no change in visibility or dependencies, a fake
surface identical to what the backend calls, no lost or weakened test, and every pairing arm
covered. Its findings and their resolution (TC-CLOSEOUT-02, one commit each):

| Finding | Severity | Resolution |
| --- | --- | --- |
| `activate_profile`'s refusal to replace a foreign active connection, and the active-device branch of `reject_foreign_interface_claim`, were never reached: disabling both left all 10 tests green | gap | `5b61f3a` adds `scenario_owned_profiles_never_replace_a_foreign_active_connection`: a foreign profile **without** `interface-name` active on a device, so only the active-device branch can refuse; and an owned bridge profile on a device where such a foreign connection is active. New controls: M (activation refusal disabled) fails with "bridge activation touched NetworkManager: `ActivateConnection`"; N (active-device branch disabled) fails with "bridge on eth5 touched NetworkManager: `AddConnection2`" |
| `ensure_address`/`ensure_route` write `Update2` to the owned profile before activation is refused there | gap (untested behavior) | the same scenario asserts the call log is exactly one `Update2` on the **owned** profile, with no `ActivateConnection` or `Reapply` and the foreign profile byte-identical and still active. This is existing behavior on BlueRoute's own profile only, so it is asserted as is, not changed |
| removal was scoped by network but not shown scoped by interface | gap | `5b61f3a`: the removal scenario now has one network owning profiles on two interfaces; removing `br-two` deactivates and deletes only that profile |
| `ensure_bridge`'s refusal of an owned non-bridge profile was not covered | gap | `5b61f3a`: same new scenario, `InvalidState` with no call |
| no bound on the child run or on `next_event()`, so a regression could hang CI | nit | `5b61f3a`: the outer test logs the child to a file and kills it after 180 s; each subscription wait fails after 10 s |
| the CLI matrix depended on the checkout path: zbus percent-encodes some characters, so a target directory with `+` failed 37 cases | nit | `01edad6`: fixed address `unix:path=/nonexistent/blueroute-behavior-no-such-bus`; the matrix passes with `CARGO_TARGET_DIR` containing `+` and a space; expected files unchanged |
| prefix 255 (named in COV-5) and the `read_u16`/`read_u32` error paths were not asserted; a helper named `v4` also parsed IPv6 | nit | `36de5c8`: 255 cases for both parsers (IPv4 and IPv6 → `InvalidInput`), a direct test of both readers at every wrong width, helper renamed `ip` |

With these changes the suite has ten scenarios and the outer test requires exactly ten to pass;
the TC-NM section above describes the original nine.

**Accepted limitations (recorded, not changed).** The fake stores `AddConnection2`/`Update2`
settings without NetworkManager's validation, ignores `Reapply` settings, never returns the
legacy `addresses`/`routes` keys real NetworkManager adds (their removal is covered by the
pre-existing unit tests `address_mutation_removes_legacy_addresses_property` and
`route_mutation_removes_legacy_routes_shadow`), keeps devices managed, and completes activation
immediately, so the `APPLY_TIMEOUT` paths of the four waits are not exercised (each would take
8 s). Run without `dbus-run-session` on a desktop, the child would claim the NetworkManager name
on the user's session bus, never the system bus; the documented command and CI always use a
private bus. Real NetworkManager behavior remains with the Part A qualification.

**Findings (COV-2), whole backlog.** No production defect was found. One observation from
TC-PARSE (an address under the wrong family group is read as valid) remains an observation: the
review raised no concern, BlueRoute writes each prefix under its own family, and no task is
opened.

**Requirement map.** Every COV and VERIFY requirement is met; see the ledger in
[TEST_COVERAGE_TODO.md](TEST_COVERAGE_TODO.md#requirement-ledger). Acceptance is software-only and
changes no product claim; installed-system and hardware qualification stay open in their own
backlogs.

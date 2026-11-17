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

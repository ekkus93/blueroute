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

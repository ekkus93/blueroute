# Test Coverage Specification

**Status:** Implemented and accepted (2026-11-16, final code candidate `36de5c8`); see the
[backlog](TEST_COVERAGE_TODO.md) and [evidence](TEST_COVERAGE_EVIDENCE.md).  
**Baseline:** `e00aa28` (2026-11-16).  
**Execution backlog:** [TEST_COVERAGE_TODO.md](TEST_COVERAGE_TODO.md).  
**Related:** [product specification](SPEC.md), [module split](MODULE_SPLIT_SPEC.md),
[shell-free audit](P14-005-SHELL-FREE-AUDIT.md).

## 1. Problem and scope

A structural survey of the baseline (production lines versus inline and integration tests, then a
read of each parser's rejection branches) found five places where code that guards ownership,
fail-closed parsing or user-visible output has little or no automated coverage. No line-coverage
tool was used; the gaps below were confirmed by reading the code and its tests.

| Gap | Location | Today |
| --- | --- | --- |
| G1: real NetworkManager backend | `crates/blueroute-linux/src/networkmanager/` (`profile.rs` 310 production lines, `inventory.rs` 293) | No automated test drives the backend over D-Bus. `network_backend_contract.rs` uses an in-process fake of the `NetworkBackend` trait; inline tests cover only settings encoding. Only the Part A VM run exercises profile create, update, activate, reapply, deactivate and delete. |
| G2: route- and address-data parsers | `networkmanager/routes.rs` `route_key_from_entry`; `networkmanager/addresses.rs` `address_prefixes` | Only the unsupported-attribute rejection is tested. Missing metric or prefix, an out-of-range prefix, an invalid destination or next hop and wrong-family values are untested. |
| G3: netlink parsers | `crates/blueroute-linux/src/kernel_address.rs` (`netlink_message`, `parse_netlink_ack`, `parse_ipv4_address`, `read_u16`/`read_u32`) | Three success-path tests. Truncated and inconsistent kernel messages, attribute-length errors, skipped families or interfaces and errno classification are untested. |
| G4: BlueZ pairing errors and agent callbacks | `crates/blueroute-linux/src/bluez/pairing.rs` | 2 of the 9 mapped BlueZ error names are tested. `request_confirmation` and `authorize_service` are not tested against an unauthorized device; `trust_error`, `trust_property_error`, `pairing_error` and `incoming_pairing_*` mappings are untested. |
| G5: CLI behavior matrix | `apps/cli/` | The 62-case behavior capture used for the module split lives only in the ignored `tmp/ms/capture.sh`. `run.rs` has no tests of its own; `cli_exit_contract.rs` checks exit statuses but not output text. |

A sixth item is housekeeping: `crates/blueroute-linux/tests/route_projection_contract.rs` holds no
tests of its own. It recompiles `src/route_projection.rs` through `#[path]`, so that module's nine
tests run twice, and its doc comment still describes the wiring as pending although
`route_projection` has been part of the library since P8-005.

**Out of scope.** Physical Bluetooth, real NetworkManager and real kernel behavior stay with the
Part A and Part B qualification backlogs; nothing here replaces or closes them. Code that is
already well covered (control TLS identity and certificates, the protocol crate, removal, control
worker and listener, reconnect and owner-race suites) is not revisited. Installing a coverage
tool is not part of this plan.

## 2. Requirements

**COV-1 — Tests only:** The work adds tests, test fixtures and test-only helpers. Production
behavior, error text, log messages, visibility (except `#[cfg(test)]` items), dependencies of
production targets and every invariant in `docs/SPEC.md` stay unchanged. Dev-dependencies may be
added only when no existing dependency serves, with the reason recorded.

**COV-2 — A failing test is a finding, not a fix:** If a new test exposes a defect, the test is
committed as `#[ignore = "COV finding <ID>: …"]` only if the fix cannot land in the same task;
otherwise the defect is recorded in the evidence and fixed in its own reviewed commit, separate
from the test commit, with `docs/SPEC.md` updated first if an invariant is affected. A test is
never weakened to pass.

**COV-3 — Fake NetworkManager over a real bus (G1):** A test-only NetworkManager service runs on
a private `dbus-daemon`. It implements only the methods and properties the backend calls at the
baseline:

- `org.freedesktop.NetworkManager`: `Version`, `GetDevices`, `ActivateConnection`,
  `DeactivateConnection`;
- `…Settings`: `ListConnections`, `AddConnection2`;
- `…Settings.Connection`: `GetSettings`, `Update2`, `Delete`;
- `…Device`: `Interface`, `DeviceType`, `Managed`, `State`, `ActiveConnection`, `Reapply`;
- `…Connection.Active`: `Connection`.

*Correction (2026-11-16, TC-NM):* the list originally placed `State` on
`…Connection.Active` and omitted it from `…Device`. The backend reads the device's `State`
(`inventory.rs` `read_device`) and only `Connection` from the active connection; the list above is
the corrected surface. Name ownership is checked through the bus's own `NameHasOwner` and
`GetNameOwner`, which the private `dbus-daemon` provides.

The production `NetworkManagerBackend::connect_system()` is used unchanged. Because the workspace
forbids `unsafe` (and so `std::env::set_var`), the outer test re-runs its own binary with
`DBUS_SYSTEM_BUS_ADDRESS` pointing at the private bus, as `apps/desktop/tests/watch_integration.rs`
does. The fake records every mutating call by object path.

**COV-4 — Ownership is proven at the D-Bus boundary (G1):** Against the fake, the suite shows:

1. ensure bridge / address / route creates or updates only BlueRoute-owned profiles, is
   idempotent, and leaves the fake's foreign profiles byte-identical;
2. remove deletes only the owned profile for the requested NetworkId and interface;
3. a foreign profile, a profile owned by another NetworkId, a profile with malformed ownership
   metadata and two profiles claiming the same ownership each make the operation fail closed
   with **no** `Update2`, `Delete`, `Reapply`, `ActivateConnection` or `DeactivateConnection` call
   on any foreign or ambiguous path;
4. NetworkManager being absent from the bus, or a method returning a D-Bus error, maps to the
   documented `CoreError` kind without a panic or a partial mutation of other profiles;
5. inventory (`network_connections`, `network_devices`, `addresses`, `routes`) reports exactly
   what the fake holds.

Waits that poll for profile or device state must complete within bounded test time; the fake
makes state transitions immediate or scripted, never timing-dependent.

**COV-5 — Parser rejection tables (G2, G3):** Every rejection branch in `route_key_from_entry`,
`address_prefixes`, `netlink_message`, `parse_netlink_ack` and `parse_ipv4_address` has a
table-driven case asserting both that the input is rejected and the `ErrorKind`. Accepting
branches that skip input (non-IPv4 family, other interface index, unknown attribute, empty
next hop) are asserted to skip rather than fail. Boundary values are included: prefix 32/33 and
128/129 where relevant, prefix 255/256, header length exactly 16, attribute length exactly 4, and
attribute lengths that are not multiples of four.

**COV-6 — Pairing error mapping and agent refusal (G4):** Every arm of `pairing_method_error`,
including the default, is asserted with its `ErrorKind` and message. The other error mappers
(`pairing_error`, `trust_error`, `trust_property_error`, `incoming_pairing_error`,
`incoming_pairing_property_error`, `pairing_agent_registration_error`) each get at least one case
per distinct `ErrorKind` they can return. Every agent callback that takes a device is asserted to
refuse a device outside the active permit or incoming window.

**COV-7 — Checked-in CLI behavior matrix (G5):** The 62-case matrix becomes a normal test in
`apps/cli/tests/`. It runs the built binary with `DBUS_SYSTEM_BUS_ADDRESS` pointing at a socket
that does not exist and compares stdout, stderr and exit status with expected files checked in
under `apps/cli/tests/behavior/`. A mismatch prints the case and a diff. Expected files are
regenerated only by an explicit opt-in (an environment variable), never automatically. The test
needs no private bus and runs in plain `cargo test`.

**COV-8 — Remove the duplicate route projection harness:** Delete
`crates/blueroute-linux/tests/route_projection_contract.rs` after confirming that every test it
compiles is the library's own `route_projection::tests` and still runs from `--lib`. Record the
before and after harness listings.

**COV-9 — Scheduling and documentation:** Any new `#[ignore]` private-bus suite is added to
`.github/workflows/ci.yml` and to the integration list in `CLAUDE.md` in the same change, in the
existing style (`dbus-run-session -- cargo test -p <pkg> --test <file> --locked -- --ignored`).

## 3. Verification

**VERIFY-1 — Per task:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
--locked -- -D warnings`, `cargo test --workspace --locked`, and for G1 the new broker suite run
three times in a row without failure.

**VERIFY-2 — Tests detect what they claim:** Each task records at least one negative control: a
temporary, uncommitted mutation of the code under test (for example, dropping the ownership check
before `Delete`, accepting a missing metric, mapping `AuthenticationRejected` to `PairingFailed`,
or changing one CLI output word) that makes the new tests fail. The control runs in a scratch
worktree under `tmp/`, never on master, and must abort loudly if its edit does not apply.

**VERIFY-3 — Counts:** Record the harness listing before and after each task. Test names only
grow, except the nine duplicate names removed by COV-8.

**VERIFY-4 — Exact candidate and review:** After all tasks, run the full workspace checks, every
broker suite in `ci.yml` (including the new one), Python and frontend tests, and read
exact-revision CI with `gh`. An independent reviewer checks that no production line changed
(`git diff <baseline>..HEAD -- '*.rs'` touches only test code, test modules and new test files),
that the fake implements nothing beyond COV-3, and that the COV-4 foreign-path assertions are
actually reached.

## 4. Evidence and closure

Record baseline and final listings, negative controls with their failing output, any findings
under COV-2, commit SHAs, validation results and CI in `docs/TEST_COVERAGE_EVIDENCE.md`, and map
every requirement to tasks and evidence. Acceptance is software-only and changes no product
claim; the fake NetworkManager suite is evidence about the backend's D-Bus use, not about real
NetworkManager behavior, which stays with Part A.

# Test Coverage TODO

**Status:** COMPLETE (2026-11-16, final code candidate `36de5c8`, CI 44040584052); software-only, tests only.  
**Baseline:** `e00aa28`.  
**Companion specification:** [TEST_COVERAGE_SPEC.md](TEST_COVERAGE_SPEC.md).  
**Implementation evidence:** [TEST_COVERAGE_EVIDENCE.md](TEST_COVERAGE_EVIDENCE.md).

## Tracking

- Stable `TC-*` IDs. `[ ]` pending, `[-]` in progress, `[x]` accepted, `[!]` blocked with a reason.
  Keep parents and children consistent; tick an item only when its acceptance is evidenced.
- Tests only (COV-1). If a new test finds a defect, follow COV-2: record it, and fix it in its
  own reviewed commit, separate from the test commit.
- Negative controls and listings go in the ignored workspace `tmp/`; the durable record is the
  evidence file.
- Commit directly to master with Conventional Commits scoped to the task (`test(TC-NM): …`), run
  `cargo fmt --all` first, push, and verify CI on the exact pushed head with `gh`. Wait for a
  candidate's CI before pushing the next one.

## Task register

| Task | Outcome | Gap | Dependencies | State |
| --- | --- | --- | --- | --- |
| TC-BASE | Baseline listings and evidence file | — | None | `[x]` |
| TC-PARSE | Route/address and netlink parser rejection tables | G2, G3 | TC-BASE | `[x]` |
| TC-PAIR | BlueZ pairing error mapping and agent refusal | G4 | TC-BASE | `[x]` |
| TC-CLI | Checked-in CLI behavior matrix | G5 | TC-BASE | `[x]` |
| TC-NM | Fake NetworkManager broker suite | G1 | TC-BASE | `[x]` |
| TC-DEDUP | Remove the duplicate route projection harness | — | TC-BASE | `[x]` |
| TC-VALIDATE | Exact-candidate validation and CI | all | all above | `[x]` |
| TC-CLOSEOUT | Review, requirement map and closure | all | TC-VALIDATE | `[x]` |

Order: the cheap, self-contained unit tests first (TC-PARSE, TC-PAIR), then TC-CLI, then the
largest task, TC-NM. TC-DEDUP is independent and can go anywhere after TC-BASE.

## TC-BASE — Record the baseline

**Requirements:** VERIFY-3.

- [x] **TC-BASE-01:** Record HEAD, dirty state and toolchain. Confirm the gap table in the
  specification still matches the code (function names, test counts per file).
- [x] **TC-BASE-02:** Capture sorted harness listings: `cargo test -p blueroute-linux --lib
  --locked -- --list`, `cargo test -p blueroute-linux --tests --locked -- --list`, `cargo test -p
  blueroute --locked -- --list`.
- [x] **TC-BASE-03:** Create `docs/TEST_COVERAGE_EVIDENCE.md` with the baseline counts and how
  each later comparison is made.

**Acceptance:** Baseline listings recorded; gap table confirmed or corrected with a dated note.

## TC-PARSE — Parser rejection tables

**Requirements:** COV-1, COV-2, COV-5, VERIFY-1/2/3.

- [x] **TC-PARSE-01:** In `networkmanager/routes.rs` tests, add a table for
  `route_key_from_entry`: unsupported key; missing, non-string or invalid `dest`; IPv4 entry under
  IPv6 and vice versa; missing, non-integer, 256 and family-invalid `prefix`; invalid and
  wrong-family `next-hop`; empty `next-hop` accepted as no gateway; missing and non-integer
  `metric`. Assert `ErrorKind` for each rejection.
- [x] **TC-PARSE-02:** In `networkmanager/addresses.rs` tests, add the equivalent table for
  `address_prefixes`, covering every `CoreError` branch in the function.
- [x] **TC-PARSE-03:** In `kernel_address.rs` tests, add tables for `netlink_message` (under 16
  bytes, declared length under 16, declared length over the buffer, exactly 16),
  `parse_netlink_ack` (under 4 bytes, 0, `-EEXIST` → `InvalidState`, another errno →
  `NetworkBackendUnavailable`, `i32::MIN`) and `parse_ipv4_address` (truncated message, non-INET
  family skipped, other interface skipped, truncated attribute header, attribute length under 4
  or past the end, unaligned lengths, non-4-byte address ignored, `IFA_ADDRESS` used when
  `IFA_LOCAL` is absent, no address → `None`).
- [x] **TC-PARSE-04:** Negative controls (VERIFY-2): for example, accept a missing metric, and
  drop the `length > bytes.len()` check; record the failing tests.

**Acceptance:** Every rejection branch named in COV-5 is asserted with its kind; controls fail
as expected; listings grow only by the new names.

## TC-PAIR — Pairing error mapping and agent refusal

**Requirements:** COV-1, COV-2, COV-6, VERIFY-1/2/3.

- [x] **TC-PAIR-01:** Table test over all nine named arms of `pairing_method_error` plus an
  unknown name, asserting `ErrorKind` and message.
- [x] **TC-PAIR-02:** Cases for `pairing_error`, `trust_error`, `trust_property_error`,
  `incoming_pairing_error`, `incoming_pairing_property_error` and
  `pairing_agent_registration_error`: at least one per distinct `ErrorKind` each can return.
- [x] **TC-PAIR-03:** For every agent callback that takes a device (`request_pin_code`,
  `request_passkey`, `request_confirmation`, `request_authorization`, `authorize_service`), assert
  refusal for a device outside the active permit and outside the incoming window, and the
  expected result for the authorized device.
- [x] **TC-PAIR-04:** Negative controls: remap `AuthenticationRejected`, and let
  `authorize_service` skip `require_authorized`; record the failures.

**Acceptance:** Every mapping arm and agent callback is asserted; controls fail as expected.

## TC-CLI — Checked-in CLI behavior matrix

**Requirements:** COV-1, COV-7, VERIFY-1/2/3.

- [x] **TC-CLI-01:** Port the 62 cases from `tmp/ms/capture.sh` (its text is preserved in
  `docs/MODULE_SPLIT_EVIDENCE.md`) into `apps/cli/tests/behavior_matrix.rs`, one named case per
  invocation, running the built binary via `CARGO_BIN_EXE_blueroute` with
  `DBUS_SYSTEM_BUS_ADDRESS` pointing at a nonexistent socket under the test's temp directory.
- [x] **TC-CLI-02:** Check in expected stdout, stderr and status under
  `apps/cli/tests/behavior/`, generated once from the baseline binary. Confirm they equal the
  module-split capture. Regeneration happens only when an opt-in environment variable is set.
- [x] **TC-CLI-03:** Mismatches report the case name and a line diff. Run the test twice to
  confirm it is deterministic.
- [x] **TC-CLI-04:** Negative control: change one help word and one JSON field name; record the
  failing cases.

**Acceptance:** The matrix runs in plain `cargo test`, passes on the baseline and fails on a
one-word output change.

## TC-NM — Fake NetworkManager broker suite

**Requirements:** COV-1, COV-2, COV-3, COV-4, COV-9, VERIFY-1/2/3.

- [x] **TC-NM-01:** Write the test-only fake service in
  `crates/blueroute-linux/tests/networkmanager_backend/` (zbus `#[interface]` objects for exactly
  the COV-3 surface), with seedable connections, devices and active connections, scripted
  failures, and a log of mutating calls by object path. Record any dev-dependency added and why.
- [x] **TC-NM-02:** Add `crates/blueroute-linux/tests/networkmanager_backend.rs`: an `#[ignore]`
  outer test that re-runs the binary with `DBUS_SYSTEM_BUS_ADDRESS` set to the private bus, and
  inner tests that use the unmodified `NetworkManagerBackend::connect_system()`.
- [x] **TC-NM-03:** Owned-path tests (COV-4 items 1, 2, 5): ensure bridge, address and route
  create then update idempotently; remove deletes only the requested owned profile; inventory
  matches the fake; foreign profiles stay byte-identical throughout.
- [x] **TC-NM-04:** Fail-closed tests (COV-4 item 3): foreign, other-NetworkId, malformed and
  duplicate-claim profiles each fail the operation, and the call log shows no mutating call on
  any of their paths.
- [x] **TC-NM-05:** Failure tests (COV-4 item 4): NetworkManager absent from the bus, and a D-Bus
  error from each mutating method, map to the documented kinds with no mutation of other profiles.
- [x] **TC-NM-06:** Schedule the suite in `ci.yml` and list it in `CLAUDE.md` (COV-9). Run it
  three times in a row locally.
- [x] **TC-NM-07:** Negative controls: skip the ownership check before `Delete`, and before
  `Update2`; record the failing tests. Confirm the foreign-path assertions are actually reached.

**Acceptance:** The suite passes repeatedly against the unmodified backend, every COV-4 item is
asserted, controls fail as expected, and CI runs the suite.

## TC-DEDUP — Remove the duplicate route projection harness

**Requirements:** COV-8, VERIFY-3.

- [x] **TC-DEDUP-01:** Confirm `route_projection_contract.rs` defines no tests of its own and that
  its listed tests are exactly `route_projection::tests::*` from `--lib`.
- [x] **TC-DEDUP-02:** Delete the file; record before and after listings (nine duplicate names
  gone, `--lib` unchanged).

**Acceptance:** No route projection test is lost; the duplicate run is gone.

## TC-VALIDATE — Validate the exact candidate

**Requirements:** VERIFY-4, COV-1.

- [x] **TC-VALIDATE-01:** On the exact candidate, run `cargo fmt --all -- --check`, `cargo check
  --workspace --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo
  test --workspace --locked`, the experimental-join tests, every broker suite in `ci.yml`
  (including the new one), the Python tests and the desktop frontend checks.
- [x] **TC-VALIDATE-02:** Confirm `git diff e00aa28..HEAD` touches no production code: only
  `#[cfg(test)]` modules, files under `tests/`, CI, `CLAUDE.md` and docs.
- [x] **TC-VALIDATE-03:** Push and read exact-revision CI with `gh`; record the run ID.

**Acceptance:** Every check passes on the exact candidate; CI passes.

## TC-CLOSEOUT — Review and close

**Requirements:** VERIFY-4, all COV requirements.

- [x] **TC-CLOSEOUT-01:** Independent review of the test diff: no production change, the fake
  implements only the COV-3 surface, the COV-4 foreign-path assertions are reached, no test is
  weakened or flaky.
- [x] **TC-CLOSEOUT-02:** Resolve review findings, each in its own commit.
- [x] **TC-CLOSEOUT-03:** Complete the requirement ledger below and the evidence file; record any
  COV-2 findings and their fixes.
- [x] **TC-CLOSEOUT-04:** Mark this backlog COMPLETE with the candidate SHA and CI run.

**Acceptance:** Reviewed, test-only change set with all requirements met and evidenced.

## Requirement ledger

| Requirement | Tasks | Evidence | State |
| --- | --- | --- | --- |
| COV-1 Tests only | all | [TC-VALIDATE](TEST_COVERAGE_EVIDENCE.md#tc-validate--exact-candidate-validation-2026-11-16) | MET |
| COV-2 Findings handled separately | all | [TC-CLOSEOUT](TEST_COVERAGE_EVIDENCE.md#tc-closeout--review-and-close-2026-11-16) | MET |
| COV-3 Fake NetworkManager over a real bus | TC-NM | [TC-NM](TEST_COVERAGE_EVIDENCE.md#tc-nm--fake-networkmanager-broker-suite-2026-11-16) | MET |
| COV-4 Ownership at the D-Bus boundary | TC-NM | [TC-NM](TEST_COVERAGE_EVIDENCE.md#tc-nm--fake-networkmanager-broker-suite-2026-11-16) | MET |
| COV-5 Parser rejection tables | TC-PARSE | [TC-PARSE](TEST_COVERAGE_EVIDENCE.md#tc-parse--parser-rejection-tables-2026-11-16) | MET |
| COV-6 Pairing mapping and refusal | TC-PAIR | [TC-PAIR](TEST_COVERAGE_EVIDENCE.md#tc-pair--pairing-error-mapping-and-agent-refusal-2026-11-16) | MET |
| COV-7 CLI behavior matrix | TC-CLI | [TC-CLI](TEST_COVERAGE_EVIDENCE.md#tc-cli--checked-in-cli-behavior-matrix-2026-11-16) | MET |
| COV-8 Remove duplicate harness | TC-DEDUP | [TC-DEDUP](TEST_COVERAGE_EVIDENCE.md#tc-dedup--duplicate-route-projection-harness-removed-2026-11-16) | MET |
| COV-9 Scheduling and docs | TC-NM | [TC-NM](TEST_COVERAGE_EVIDENCE.md#tc-nm--fake-networkmanager-broker-suite-2026-11-16) | MET |
| VERIFY-1..4 | all | per-task sections, [TC-VALIDATE](TEST_COVERAGE_EVIDENCE.md#tc-validate--exact-candidate-validation-2026-11-16), [TC-CLOSEOUT](TEST_COVERAGE_EVIDENCE.md#tc-closeout--review-and-close-2026-11-16) | MET |

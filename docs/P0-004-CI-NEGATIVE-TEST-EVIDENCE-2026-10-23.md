# P0-004 — Deliberate Rust test failure detection

**Date:** 2026-10-23 UTC  
**Source:** `0706756c37826dff9692487ca08f138ba5b861a9` (direct-to-`master`)  
**CI:** [39920496143](https://github.com/ekkus93/blueroute/actions/runs/39920496143) — **passed**

## Intent

P0-004 requires evidence that CI detects deliberate test failures, as well as formatting failures. A permanently red `master` run would provide little ongoing protection. Instead, the pipeline now asserts the underlying Rust test harness does report an intentionally failed assertion correctly, while treating *that observed failure* as success of the diagnostic probe.

## Implementation

- `crates/blueroute-protocol/tests/ci_failure_probe.rs` contains an intentionally panicking, `#[ignore]`-marked integration test named `deliberate_ci_failure_probe`. Normal `cargo test --workspace --locked` compiles the test and skips its execution.
- `.github/workflows/ci.yml` executes this exact test with `--ignored --exact` after the normal workspace suite. The command must exit unsuccessfully, and its captured output must identify the named test as `FAILED` and `1 failed`.
- If the intentionally failing fixture unexpectedly passes, or if the command fails for an unrelated reason (such as compilation or an unavailable test), the verification step itself fails the workflow.

The exact-head CI's **Run tests** and **Verify CI recognizes an intentional failing Rust test** steps both completed successfully.

## Scope and remaining acceptance

This probes *Rust* test failure detection on GitHub-hosted Linux runners and is not evidence of Bluetooth operation, hardware, or application UI acceptance. P0-004 remains in progress only for frontend checks when the P12 frontend is introduced. The older formatting-failure evidence remains valid. No temporary branch or PR was created.

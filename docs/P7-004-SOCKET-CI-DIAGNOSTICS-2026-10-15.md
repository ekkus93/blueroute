# P7-004 pending-handshake socket teardown: CI and write diagnostics (2026-10-15)

## Scope and security boundary

The new `apps/daemon/src/attempt_socket.rs` primitive owns actual pending TCP sockets and shuts them down before retry. It does **not** perform mutual TLS, independently approved enrollment, verified peer-key possession, or authenticated daemon runtime dispatch. The `#[allow(dead_code)]` module annotation documents that this compiled and tested primitive is not yet wired into production; it does not relax authentication. Production JoinNetwork remains fail-closed. All physical Bluetooth and multi-computer acceptance remains deferred.

## Exact-head chronology (UTC)

| UTC time | Master SHA | Evidence | Result |
| --- | --- | --- | --- |
| 2026-10-15 13:12:33 | `27822a72577c0376ac23cb6ef2c2d93bafbc867e` | Original socket-owner commit, parent `f7a68b684c30751f49c8b538640daabd344a9294`; CI [38740759891](https://github.com/ekkus93/blueroute/actions/runs/38740759891), job `116441534059`, step 6 | Failed `cargo fmt --all -- --check`; source formatting at attempt_socket.rs:8,54,71 and two extraneous blank lines in lib.rs:159,320. |
| 2026-10-15 14:17:29 | `9790c9023759a2fb397c8b3aa0b6c0e0ad7b1786` | Formatting commit; Ralph operation `e1b1580ce1064a65b92fbb49d7f0b0d5` accepted, verified two file-path receipts. CI [38750175547](https://github.com/ekkus93/blueroute/actions/runs/38750175547), job `116474233877`, step 8 | Formatting passed; Clippy failed on `dead_code` for not-yet-wired socket primitive and `nonminimal_bool` in `close_if_current`. Another run `38750088980` was cancelled. |
| 2026-10-15 14:19:54 | `238bb7488f814d54f6856bb6ecc8bd0a219a88ef` | Targeted lint fix (parent `9790c9023759a2fb397c8b3aa0b6c0e0ad7b1786`); CI [38750430778](https://github.com/ekkus93/blueroute/actions/runs/38750430778), job `116475095427` | **Passed** formatting, workspace check, Clippy `-D warnings`, tests, socket contract tests, daemon D-Bus, authorization, client, CLI, service and packaging checks. |

## Ambiguous write / execution-layer reconciliation

- During the second fix, Ralph Bridge `commit_files` was invoked for two UTF-8 text upserts (`apps/daemon/src/lib.rs` and `apps/daemon/src/attempt_socket.rs`) against exact expected head `9790c9023759a2fb397c8b3aa0b6c0e0ad7b1786`. No secret values were supplied. Full request bodies were not logged; the first formatting commit had 23,831 total UTF-8 bytes across two paths.
- The caller received exactly: `INVALID_ARGUMENT: Error code: INVALID_ARGUMENT; Error: RuntimeException: Error calling MCP tool: [TextContent(type='text', text='Error executing tool commit_files', annotations=None, meta=None)]`. No successful operation ID or path receipts were returned for that second invocation. **This was not evidence that GitHub rejected the write.**
- A read-only `get_repo_state(master)` immediately established the new commit `238bb7488f814d54f6856bb6ecc8bd0a219a88ef`, with the expected parent and message. `get_diff(9790c902..238bb748)` confirmed exactly the intended two file changes: simplify `is_none_or` and annotate the not-yet-wired module. No blind retry was performed. CI run `38750430778` then passed.
- Failure category: **ambiguous MCP/execution-layer response after an accepted GitHub mutation**, not an authorization denial or a source-code error. The original formatting/Clippy failures were separate, ordinary CI/source issues. This does not establish the root cause of the execution-layer response.

## Bridge identity and evidence limits

- Ralph Bridge runtime at inspection: semantic version `0.6.2`, build `0acea4b23362e2ccecab8f6c9db4418b162426cf`, server instance `51c945dd809b4207800c3ba33151ad94`, started `2026-10-15T02:25:51.132200Z`, authentication mode `app`. These are secret-free identifiers.
- CI job log windows were read from job `116441534059` lines 105–214 and job `116474233877` lines 189–323. No source truncation warning affected the targeted failure windows. The exact second-write request ID, stack beyond the returned exception, and mutation receipt were unavailable; do not invent them.
- Every future write must compare current exact `master` head before mutation and reconcile a failed/ambiguous response through branch state and changed paths before retrying.

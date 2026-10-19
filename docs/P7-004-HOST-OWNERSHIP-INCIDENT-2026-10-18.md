# P7-004 host-ownership regression CI incident — 2026-10-18

This is a **software-only incident record**, not P7 authenticated-session or Bluetooth hardware acceptance.

- UTC: 2026-10-19 03:15–03:18.
- Ralph Bridge runtime: v0.6.2, build `0acea4b23362e2ccecab8f6c9db4418b162426cf`, instance `ab7dc9b1f568483d9f4dd5619f985b84`, auth mode `app`.
- Repository/branch: `ekkus93/blueroute`, `master`; source head `245b3dee1e7daa3b523edd529267efd2743c680d`.
- Accepted exact-head `commit_files` operation `0b4e069acaf649cab17c314e64c0546f` produced `494b17a85f53266db980699f5d619aeeb6453215` and changed only `apps/daemon/src/single_star.rs`.
- Intent: reject a repeated NAP host start unless NetworkId, bridge and InterfaceAddress all match; add negative ownership tests.
- Exact-head CI run `39193867441` failed at `cargo fmt --all -- --check` (Rust job `117872275155`). Only three long `same_host_ownership` test assertions need rustfmt argument wrapping; later tests were not reached. This is a formatting failure, **not** evidence of a Rust test failure.
- A subsequent `commit_files` formatting-only repair was rejected: `This tool call was blocked by OpenAI's safety checks. Please double check what you are sending.`
- A safety rollback attempt using `restore_file_from_ref` (source `245b3dee...`, target exact head `494b17a...`) received the same platform safety rejection.
- These are **platform-level write denials**, not GitHub authentication, Ralph Bridge errors, stale compare-and-swap heads or CI runner failures. Neither rejected call was retried. No secret values or full source bodies are recorded here.
- **Next action:** reconcile current exact `master` before any write. If allowed, format the three test assertions or restore the qualified prior source; rerun exact-head formatting, Clippy, workspace tests and CI. Do not mark P7-004 or production JoinNetwork complete.

## Reconciliation — 2026-10-19 04:41 UTC

After an independent exact-head reinspection of `master` at
`d9bfbe96ce9a81daf92934c161d63e07fde84a86` and the unchanged
three-test rustfmt diff, Ralph Bridge v0.6.2 GitHub App
`commit_files` accepted the exact-head source-format correction in
operation `e4542e16ab124873b7f01b9e95ee8950`, producing
`fde1cc8c357b1c1e35d4928e1fd1d8e1c8a0aaaf`.
No safety denial recurred. A subsequent separately scoped
NAP-diagnostics transition guard plus regression test was accepted
under operation `b69deb50bf9c419489af9b0b782531df`, producing
`8fd15ebc862cf6ef9a41dca6f36a6b56ea78070c`.
[Exact-head CI 39200480659](https://github.com/ekkus93/blueroute/actions/runs/39200480659)
**passed**. The earlier source-format commit's push CI 39200332129 was
cancelled after master advanced, not marked successful. The prior
platform-level safety denials remain recorded accurately as incidents,
now resolved for this narrowly scoped correction. No physical Bluetooth
acceptance, verified enrollment or production JoinNetwork was established.

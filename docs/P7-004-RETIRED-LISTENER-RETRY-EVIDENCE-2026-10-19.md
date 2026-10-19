# P7-004 retired-listener retry qualification and write incident

**UTC observation:** 2026-10-19, approximately 08:34 UTC.
**Repository:** `ekkus93/blueroute`; `master`.
**Code commit:** `9af580b916b78e774aa56abb15505562ea4fb5c1`.
**Exact-head CI:** [39223329834](https://github.com/ekkus93/blueroute/actions/runs/39223329834) — passed.

## Qualified behavior

The fresh-ownership listener polling seam now refuses a repeated poll on an
already retired listener **before** calling any ownership observation callback.
A negative regression asserts zero post-retirement callbacks. This prevents a
stale daemon loop from querying the backend after listener retirement.
Earlier implementation `99688c53bdc85c729211dc580c0572aa7ce161c2`
passed [CI 39222431265](https://github.com/ekkus93/blueroute/actions/runs/39222431265):
fresh observations before and after accepting an untrusted TCP candidate,
failure-triggered listener retirement, and idle-poll behavior.

**Acceptance boundary:** This is a library primitive, not the production
daemon listener supervisor, authenticated TLS/PAN session, independently
approved key enrollment, production JoinNetwork, or physical Bluetooth
qualification. P7-003, P7-004 and P6-004 remain open.

## Secret-free platform write-denial incident

- Time: 2026-10-19, approximately 08:34 UTC.
- Runtime: Ralph Bridge `0.6.2`; build `0acea4b23362e2ccecab8f6c9db4418b162426cf`;
  instance `ecf828a193874460b2f7b3088fab8c5b`; auth mode `app`.
- Method: `commit_files`, one UTF-8 upsert, path `docs/TODO.md`,
  target `master`, exact-head compare-and-swap.
- Expected master: `9af580b916b78e774aa56abb15505562ea4fb5c1`.
- Observed master after denial: `9af580b916b78e774aa56abb15505562ea4fb5c1`.
- Exact rejection: `This tool call was blocked by OpenAI's safety checks. Please double check what you are sending.`
- Classification: platform safety rejection, **not** a Ralph Bridge,
  GitHub, CI, or rate-limit error. The rejected large TODO upsert was not
  blindly retried; no alternate representation of the same blocked payload
  was submitted.
- Reconciliation: the code commit and its exact-head passing CI are
  independently verified. The attempted TODO evidence edit was **not**
  committed. This incident note is a separate, bounded diagnostic record.

The authoritative TODO should be updated only when a permitted write can
safely preserve the complete file; never truncate or mark physical gates passed.

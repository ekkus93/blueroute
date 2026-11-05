---
name: verify-task
description: Check a docs/TODO.md task's acceptance criteria against the code, tests, and docs before it is marked complete. Usage /verify-task P4-003
disable-model-invocation: true
---

Verify task `$ARGUMENTS`:

1. Find the task in `docs/TODO.md` and read its subtasks and acceptance criteria. Read any per-task doc (`docs/$ARGUMENTS-*.md`) and the relevant `docs/SPEC.md` invariants.
2. For each criterion, locate the implementing code and the test that proves it. Note any criterion with no code, no test, or that needs physical hardware evidence.
3. Check that hardware-dependent claims have evidence in `docs/*-HARDWARE-EVIDENCE-*.md` with computer, adapter, distro, kernel, BlueZ version, network backend, and procedure. CI is not a substitute.
4. Check terminology (Supported / Tested / Experimental / Unsupported) isn't overstated.
5. Report per criterion: met / not met / unverifiable, with file references. Recommend the correct status marker (`[ ]`, `[-]`, `[x]`, `[!]`) but do not edit `docs/TODO.md` unless asked.

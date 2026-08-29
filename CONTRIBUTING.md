# Contributing to BlueRoute

BlueRoute is a Linux Bluetooth PAN networking project. The project is hardware-agnostic: a successful test on one computer or Bluetooth controller is evidence for that tested combination, not a universal support claim.

## Start here

1. Read `docs/SPEC.md` for architecture and invariants.
2. Read `docs/TODO.md` for the ordered task backlog and acceptance criteria.
3. Follow `docs/DEVELOPMENT.md` to prepare a development environment and run checks.
4. Keep each change scoped to the smallest sensible TODO task or coherent group of tasks.

## Task IDs

TODO items use stable IDs such as `P4-003`. Reference the relevant ID in commits, pull requests, test evidence, and architecture decisions when practical. Do not mark a task complete merely because code exists; its acceptance criteria must also be satisfied.

## Tests and evidence

Deterministic tests should run without physical Bluetooth hardware whenever possible. Core state machines, topology algorithms, address planning, protocol parsing, and most adapter translation logic belong in this category.

Physical Bluetooth behavior must be validated separately on real Linux systems. Hardware evidence should record the computer, Bluetooth adapter/controller, Linux distribution, kernel, BlueZ version, network backend, and exact test procedure. Never infer a global peer limit, throughput claim, or feature guarantee from one adapter.

## Platform terminology

BlueRoute documentation uses these terms consistently:

- **Supported:** part of the declared release support matrix and covered by the required acceptance evidence.
- **Tested:** demonstrated on a recorded hardware/software combination, but not necessarily covered by the full support commitment.
- **Experimental:** expected to be incomplete, unstable, or subject to interface changes.
- **Unsupported:** known not to meet requirements or intentionally outside the current support matrix.

A platform may be technically compatible before it is formally supported.

## Architectural discipline

Front ends must not implement their own Bluetooth or routing logic. Hardware-specific behavior belongs behind capability and Linux adapter boundaries. Changes that intentionally alter an invariant in `docs/SPEC.md` should update the specification before or with the implementation.

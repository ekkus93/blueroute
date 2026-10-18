# P6-007 — Multi-client NAP admission policy (software evidence)

Date: 2026-10-17
Qualified source: `42cede7a93b5bd5fe8e1abbbd06fbc6d48f518dd`
Exact-head CI: [39064441580](https://github.com/ekkus93/blueroute/actions/runs/39064441580) — passed.

`crates/blueroute-core/src/nap_admission.rs` exports a deterministic,
hardware-independent `plan_nap_client_admission` function. It accepts one
node's local `NodeCapabilities`, a caller-supplied set of active client
identities, and an admission candidate. Unknown or unusable NAP/adapter
capabilities and absent client ceilings fail closed. The source of an
explicit ceiling (measured, configured or conservative default) is retained
in the result: no model name or universal Bluetooth controller count is
inferred. Zero ceilings, duplicate/zero identities and already exceeded
capacities are rejected. A new client cannot exceed the ceiling, while an
already-attached peer is idempotently admitted without occupying a second
slot.

Five deterministic tests verify provenance/capacity arithmetic, full-star
idempotence and rejection, unknown/unusable/zero capabilities, invalid
identities and over-capacity observations, and ordering independence.

## Not yet implemented or accepted

This is **software-only policy groundwork**, not a complete P6-007
implementation. The caller must independently authenticate membership and
attachment observations and serialize admissions at the actual daemon/PAN
connection boundary. It does not open Bluetooth connections, implement
concurrent TCP/UDP traffic, verify a hardware controller's simultaneous
connection ceiling, or claim two/three-device physical behavior.

Further acceptance: production P7/P6-004 authenticated PAN integration,
parallel multi-client connections using one NAP, tested traffic between
clients, capacity/diagnostics through the daemon CLI and frontends, and
documented measured per-adapter limits when physical testing resumes.
All three P6-007 acceptance subtasks remain unchecked.

## Read-only capacity diagnostics (2026-10-17)

The `blueroute capability show` human-readable CLI output now explicitly
labels the effective NAP policy ceiling and its provenance, including whether
it is only a conservative default rather than a controller measurement.
It also states that the *live attached PANU count is unavailable* instead of
presenting a guessed zero. `--json` preserves the existing typed capability
response without inventing new daemon fields. Unknown ceilings remain
unknown. A deterministic formatter/JSON round-trip test passed [exact-head
CI 39064822988](https://github.com/ekkus93/blueroute/actions/runs/39064822988)
at `c29f09760a82e57ec07e92031ef1995a4c89a238`.

This is read-only reporting of configured/runtime policy inputs, not
authenticated peer inventory, actual multi-client session counts or proof
of controller concurrency. P6-007 still requires live runtime admission,
real concurrent connections/traffic, and hardware acceptance.

## Generation-scoped admission reservations (2026-10-17)

`crates/blueroute-core/src/nap_admission_ledger.rs` now serializes
in-process reservations and active modeled PANU slots with a mutex. Only
independently authenticated, network-approved NodeIds may be supplied by
a future daemon integration. Reservations consume capacity before attachment;
duplicate pending attempts and excess slots fail closed. A matching ticket
promotes to active, and exact-generation retirement ignores stale callbacks.
Four deterministic tests, including a 16-thread race for three slots,
passed [exact-head CI 39066719628](https://github.com/ekkus93/blueroute/actions/runs/39066719628)
at `7e56fb825040de1316846210851ae64ab0c5db57`.

## Capability-change admission refresh (2026-10-17)

`NapAdmissionLedger::update_capabilities` now atomically replaces the
sourced local adapter/NAP/client-ceiling policy under the *same mutex* as
reservation decisions. After a downgrade below existing occupancy, new
reservations are rejected without erasing active or pending generation
tickets; unknown capacity denies new reservations. Once old tickets are
retired, a valid lower ceiling can admit only its remaining slots. Tests
cover downgrade, pending and active preservation, unknown-policy denial,
retirement and recovery. [Exact-head CI 39070085859](https://github.com/ekkus93/blueroute/actions/runs/39070085859)
passed at `520d1dcc5bb48707a69ae5b5c672427f0a4e5ee8`.

**Unwired limitation:** The production daemon does not yet own this ledger
or call its policy-refresh method. The runtime must independently authenticate
PANU peers, cancel/reconcile actual in-flight attempts after capability loss,
observe attachments and release tickets on real teardown. This is not a
measured radio limit, physical multi-client test or proof of concurrent
TCP/UDP traffic. Keep all three original P6-007 subtasks open.

## Post-reservation capability revalidation (2026-10-17)

A previously issued pending ticket must now pass the *current* sourced
adapter/NAP/ceiling policy at activation, under the same ledger mutex. This
closes the downgrade race in which a ticket reserved before adapter loss or
capacity reduction could otherwise promote despite the new policy. A
downgrade below active-plus-pending occupancy refuses activation until the
owner retires excess pending tickets; an unknown ceiling also refuses it.
Failed promotion preserves the ticket for explicit cancellation/reconciliation
rather than silently claiming the physical connection disappeared. A new
deterministic regression covers both failure paths and recovery.

[Exact-head CI 39070431318](https://github.com/ekkus93/blueroute/actions/runs/39070431318)
passed at `081b15915daa189b59a0ea09ab34f6c354deb892`.
The daemon still needs to wire actual authenticated attachment observation,
capability-change cancellation, socket teardown and multi-client traffic.

## Active reservation retention across TLS loss (2026-10-17)

`apps/daemon/src/nap_control_admission.rs` now defines
`RegisteredNapControlOwner`, which couples a registered, pin-verified TLS
session to an exact network/owner/generation-scoped NAP reservation. Pending
capacity is released when TLS terminates, but an activated slot remains
accounted for until the runtime reports independently observed PANU detach
or completed owned NAP shutdown. The underlying `AuthenticatedNapLease`
still releases on drop: the running daemon must **retain** its owning object
for the life of the attachment and supply genuine detach observations.
The current production daemon does not yet wire this lifecycle to BlueZ.

The source landed at `ea0224cf75cf447ce86a4be8cd2fcb634940bbf6`.
Its initial CI run 39082117795 failed at Rust formatting, and the formatting
repair `93fb4f185b3b52413e408da4412deec713e74a51` exposed an unused
public API export in Clippy. Re-exporting the ownership type at
`2251855bc55a7298502a195c0fd31acaa2552b67` resolved strict lint.
[Exact-head CI 39088779662](https://github.com/ekkus93/blueroute/actions/runs/39088779662)
passed at that commit.

This is **software-only** qualification. It does not demonstrate a live PAN
listener, real Bluetooth attachment tracking, user-approved key enrollment,
authenticated production JoinNetwork, concurrent application traffic or a
measured hardware peer ceiling. P6-007 remains IN PROGRESS.

## Unexpected TLS-owner drop retains observed PANU occupancy (2026-10-17)

An authenticated control-session owner can be destroyed by a task failure or
ordinary Rust unwinding while its PANU remains physically attached. Previously
that destruction also dropped its active reservation lease and falsely freed
capacity. The `RegisteredNapControlOwner` destructor now retains an already
observed attachment in the generation-scoped admission ledger, while an
unactivated pending reservation still releases normally. A cloneable exact
`AuthenticatedNapTicket` can be saved by the daemon for independently
confirmed later detach; retirement ignores stale generations after replacement.
An unobserved detach leaves the slot conservatively occupied until explicit
reconciliation or full owned NAP shutdown, rather than overbooking the radio.

Two deterministic regressions cover retained active occupancy, pending cleanup,
exact-generation detach and stale callback rejection. The first implementation
at `20b1caf8461509c01253ec78770bf69d69281a8b` failed strict Clippy on a
collapsible conditional; the corrected implementation and public detach-ticket
export passed [exact-head CI 39089631145](https://github.com/ekkus93/blueroute/actions/runs/39089631145)
at `a6be95912f48a8f162f503945e0c78502c2e319d`.

**Remaining:** This is still a software-only lifecycle guard. No production
BlueZ attachment observation, independently confirmed detachment event,
user-verified TLS key enrollment, concurrent PANU traffic, or physical controller
capacity measurement was performed. The running daemon must wire real NAP/PAN
ownership, persist or retain exact tickets for teardown, and keep P6-007 open.

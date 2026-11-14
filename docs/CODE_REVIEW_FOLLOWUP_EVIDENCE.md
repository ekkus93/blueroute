# Code Review Follow-up Evidence

Durable evidence for [CODE_REVIEW_FOLLOWUP_TODO.md](CODE_REVIEW_FOLLOWUP_TODO.md); requirements in
[CODE_REVIEW_FOLLOWUP_SPEC.md](CODE_REVIEW_FOLLOWUP_SPEC.md). Entries are appended as tasks are
accepted; earlier entries are not rewritten. CI is checked with the GitHub CLI (`gh run`), per the
maintainer decision recorded in the TODO.

## FU-BASE — baseline and failing expectations (2026-11-13)

**FU-BASE-01.** HEAD at the start was `df02d09` = review baseline `6d200d0` plus the two follow-up
documents (no source difference). Untracked only: `.serena/`, `AGENTS.md` (never committed).
Toolchains: rustc 1.95.0, Node 26.13.0, npm 11.20.0. Guidance read: `CLAUDE.md`, `AGENTS.md`, the
follow-up specification, the first remediation specification and backlog.

**FU-BASE-02/03.** Each reproduced finding has a repository test stating the corrected contract,
committed `#[ignore]` with an `FU-xx regression` reason. Bus tests are in their own files, so CI's
`--ignored` runs of the existing suites stay green; each fix task enables its tests and adds them
to CI.

| Finding | Test | Command | Observed failure at the baseline |
| --- | --- | --- | --- |
| REV-01 | `apps/daemon/tests/publisher_progress.rs` `fu_01_a_pending_network_read_does_not_hold_back_a_health_change`: real `DaemonService` + production publisher on a private bus; the network list is held pending past one network recheck, then `update_runtime_observation` publishes `Degraded` | `dbus-run-session -- cargo test -p blueroute-daemon --test publisher_progress --locked -- --ignored` | "no StateChanged within 1s of a published health change while the network read was pending" |
| REV-02 (a) | `crates/blueroute-client/tests/event_owner_race.rs` `fu_02_an_event_whose_owner_hands_over_during_its_version_check_is_not_delivered`: A emits, then hands the name to compatible B inside that event's own Version call (call 3), staying connected | `dbus-run-session -- cargo test -p blueroute-client --test event_owner_race --locked -- --ignored` | "A's obsolete event was not retired: Some(Ok(Event(HealthChanged(Degraded))))" |
| REV-02 (b) | same file, `fu_02_owner_loss_is_reported_while_the_events_version_reply_is_held`: as (a), A then holds its reply until released | same | "owner loss not reported within 2 s of the handover: None" |
| REV-03 | `crates/blueroute-core/src/topology.rs` `fu_03_a_segment_cloned_from_another_graph_brings_no_phantom_members`: a populated segment cloned into a graph (a) without the client and (b) with the client but no link | `cargo test -p blueroute-core --lib --locked fu_03 -- --ignored` | "member NodeId([2, …]) is not a node" |
| REV-04 | `apps/daemon/src/removal.rs` `fu_04_unresolved_client_teardown_fails_and_keeps_its_proof_for_the_retry`: recorded client with owned profile/address and live owned PANU, plus a host-only network; cleanup without Bluetooth, then with it, then again | `cargo test -p blueroute-daemon --lib --locked fu_04 -- --ignored` | "unobserved client teardown reported success: RemovalReport { networks: 2, failed_networks: 0, remaining: 0, unresolved: 0, bluetooth: false }" |

The existing removal test `a_proven_owned_panu_link_is_disconnected_first_and_only_with_bluetooth`
still accepts success without Bluetooth; FU-04-04 corrects it. GAP-01 (startup baseline) and
GAP-02 (qualification fixture) are not reproduced defects: GAP-01 gets barrier tests in FU-05,
GAP-02 is a procedure correction in FU-06.

**FU-BASE-04.** Fixtures: a held network read and a held Version reply both poll a flag that a
drop guard releases, so a failing assertion cannot hang the test or the publisher's shutdown; each
waits with an explicit bound; ownership is explicit NetworkIds (known, foreign, other network);
bus tests serialize on a mutex because they share the well-known name.

**FU-BASE-05.** Isolation: bus tests use the private session bus of `dbus-run-session`; state in
temporary directories; removal tests use the fake backend; the isolated dpkg test replaces the
installed maintenance helper with a mock before any `prerm remove` runs (install and upgrade never
call it), and its `rm` guard rejects paths outside the isolated root. `DPKG_ROOT` relocates paths
only and does not isolate D-Bus; that limitation is FU-06-04's to document in the procedure.

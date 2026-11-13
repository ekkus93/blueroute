# Code Review Remediation Evidence

Durable evidence for [CODE_REVIEW_REMEDIATION_TODO.md](CODE_REVIEW_REMEDIATION_TODO.md). The
requirements are in [CODE_REVIEW_REMEDIATION_SPEC.md](CODE_REVIEW_REMEDIATION_SPEC.md). Entries
are appended as tasks are qualified; earlier entries are not rewritten.

## CR-BASE — baseline and reproductions (2026-11-13)

### CR-BASE-01: baseline recheck

- HEAD is `4ca5042ee28743dd30782379fbcf8fa1ed15df77`, identical to the reviewed baseline.
- Working tree before remediation: only untracked `.serena/` (never committed), `AGENTS.md`, and
  the two remediation documents. No tracked file differed from the baseline.
- Applicable instructions: `CLAUDE.md` and `AGENTS.md` agree (pinned toolchains, locked checks,
  Conventional Commits with task scopes, oxlint findings fixed without disabling rules, ignored
  D-Bus tests run with `dbus-run-session`, TODO items ticked only on accepted evidence).
- The code at HEAD still matches every finding's description (confirmed by the reproductions
  below and the static reading for F5 and F7).

### CR-BASE-02/03: findings and repository reproductions

Each reproducible finding now has a repository test that states the **corrected** behavior and
fails on the baseline. The tests are committed disabled so the required checks stay green:
Rust tests carry `#[ignore = "CR-Fx regression: …"]`, the UI test uses vitest `test.fails`.
Each fix task re-enables its tests; the commit that does so is that finding's evidence. The
baseline failures below were observed at `4ca5042` plus only these test files.

| Finding | Reproduction | Command | Observed baseline failure |
| --- | --- | --- | --- |
| F1 | `apps/daemon/tests/state_events.rs` `cr_f1_a_worker_health_change_reaches_a_connected_subscriber`: a real `DaemonService` on a private bus publishes a runtime health observation through `update_runtime_observation`; a separate connection waits for the `Event` signal. | `dbus-run-session -- cargo test -p blueroute-daemon --test state_events --locked -- --ignored` | `no event reached the subscriber within 1s` (published health was already `Degraded`). |
| F2 | `crates/blueroute-client/tests/owner_watch.rs` `cr_f2_idle_daemon_loss_ends_the_event_subscription_without_any_event`: subscribe, then the daemon releases its name and disconnects without emitting an Event. | `dbus-run-session -- cargo test -p blueroute-client --test owner_watch --locked -- --ignored` | `the subscription did not notice the owner loss within 2s`. |
| F3 | `crates/blueroute-core/src/topology.rs` `cr_f3_*` (4 tests): identical segment reinsertion, segment NAP change under links, LinkId rebinding, and a whole-graph consistency check. | `cargo test -p blueroute-core --locked --lib cr_f3 -- --ignored` | Reinsertion drops the attached client; NAP change and rebinding are accepted; a link's NAP no longer matches its segment. |
| F4 | `apps/daemon/src/single_star.rs` `cr_f4_recovered_hosting_capability_unblocks_create_without_restart`: one `DaemonService` + `SingleStarNetworkOperations` (fake runtime) starts incapable, then publishes a usable observation. | `cargo test -p blueroute-daemon --locked --lib cr_f4 -- --ignored` | Status reports `can_host_pan() == Some(true)`, yet create fails `NotSupported("this Linux node does not expose BlueZ NAP capability")`; the runtime is never reached. |
| F6 | `apps/desktop/ui/src/useDaemon.test.tsx` "CR-F6: every subscription is released exactly once …": `useDaemon` under `StrictMode` with deferred `subscribe` results, resolved after the replay, then unmount. | `npx vitest run ui/src/useDaemon.test.tsx` (in `apps/desktop`; as `test.fails` it reports "expected fail") | Run as a plain test: `expected [ +0, 1 ] to deeply equal [ 1, 1 ]`: the first subscription is never released. |
| F8 | `crates/blueroute-client/tests/owner_race.rs` `cr_f8_incompatible_replacement_during_version_check_receives_no_command`: compatible owner A hands the name to API-99.0 owner B inside its request-time `Version` reply; B counts `Request` calls. | `dbus-run-session -- cargo test -p blueroute-client --test owner_race --locked -- --ignored` | B received the `CreateNetwork` request (`left: 1, right: 0`), and the client reported `Ok(Ack)` from B. |

The handover in F8 happens inside A's handler, so it is deterministic, not timing-dependent. The
F1 and F2 bounds are the spec's controlled-environment bounds (EVENT-6, WATCH-4).

### CR-BASE-04: F5 static-evidence basis

F5 is a missing cleanup path, so its basis is static, as the specification states:

- `packaging/debian/prerm` only runs `deb-systemd-invoke stop blueroute.service … || true` on
  remove/deconfigure; failures are swallowed.
- `packaging/debian/postrm` only disables the unit on remove and deletes `/var/lib/blueroute`
  on purge. Nothing removes the bridge, address or NetworkManager profile the daemon created.
- `packaging/systemd/blueroute.service` has no `ExecStop`, and `RuntimeDirectory=blueroute` means
  systemd deletes `/run/blueroute` on stop, which is where removal-time evidence such as a
  forwarding lease must survive (CLEAN-4).
- The isolated upgrade test (`packaging/debian/test-package-upgrade.sh`) mocks the systemd
  helpers and checks file removal and state retention only; it cannot show network state.

Fake-backend fixtures for owned/foreign profiles and addresses already exist
(`apps/daemon/src/client_leave.rs` tests; `crates/blueroute-linux` forwarding lease tests). The
remaining fixtures (interrupted cleanup, stopped daemon, the cleanup entry point itself) depend on
the entry-point design that CR-F5-01 must record first, so CR-BASE-04 stays open until then.

### CR-BASE-05: F7 discrepancies at the baseline

| Location | Claim | Actual |
| --- | --- | --- |
| `docs/SPEC.md` (current implementation, `blueroute-protocol`) | "API 1.5" | `ApiVersion::CURRENT` is 1.6 (`crates/blueroute-protocol/src/lib.rs`); `docs/P5-PROTOCOL.md` correctly says 1.6. |
| `docs/SPEC.md` (incomplete behavior) | "The package ships the daemon and CLI only" | `blueroute` ships daemon, CLI and TUI; `blueroute-desktop` is a separate package (`packaging/debian/build-package.sh`). |
| `docs/TODO.md` status table, P15-003 | "the desktop application is still a placeholder" | A functional desktop app is packaged as `blueroute-desktop`. |
| `docs/TODO.md` status table header and P6-004/P7-003/P7-004 rows | Dated 2026-10-16; join preflight "pending … PANU/control wiring"; enrollment UX and live transport open | Join path, inbound listener, client supervisor, reconnection and typed enrollment on both sides exist in software behind the closed join gate (later dated notes in the same file). |
| `docs/ADR-INDEX.md` | Lists only P7-001; P7-001 says "user-verified enrollment UX and live daemon transport integration remain open" | The accepted P8-009 ADR (`docs/P8-009-DYNAMIC-ROUTING-ADR.md`) is missing; typed enrollment and the daemon transport now exist in software (hardware unqualified). |
| `docs/DEVELOPMENT.md` | "The placeholder crates intentionally contain almost no behavior" | All crates are implemented. |

Membership schema v3 (`CURRENT_FORMAT_VERSION = 3`) is described correctly where stated. Dated
historical hardware evidence is correct at its recorded scope and is not a discrepancy.

### CR-BASE-06: isolation

Every new reproduction runs without the host package database, system services, Bluetooth
controllers or user networking: the D-Bus tests use a private session bus from
`dbus-run-session` (never the system bus); the daemon tests use temporary directories and the
fake star runtime; the graph tests are pure; the UI test runs in jsdom with fake APIs.

# Code Review Remediation Specification

**Status:** Proposed; no remediation is implemented or accepted by this document.  
**Baseline:** `4ca5042ee28743dd30782379fbcf8fa1ed15df77` (`master`, reviewed 2026-11-13).  
**Execution backlog:** [CODE_REVIEW_REMEDIATION_TODO.md](CODE_REVIEW_REMEDIATION_TODO.md).  
**Product architecture:** [SPEC.md](SPEC.md). **Original backlog:** [TODO.md](TODO.md).

## 1. Purpose and scope

Resolve the eight findings from the repository-wide code review, preserve existing security and
ownership guarantees, and add integration coverage at the boundaries the review found defective.
This specification defines observable outcomes, implementation constraints, and closure evidence.

The review reproduced F1, F2, F3, F4, F6, and F8 using isolated D-Bus, public-API, or React probes.
F5 is a missing cleanup path established by static inspection; its precise live-system residue
still requires qualification. F7 is directly observable documentation drift. Passing baseline
checks did not invalidate these findings: 854 default Rust tests, 34 frontend tests, five private
D-Bus checks, package builds, and isolated package upgrades passed during the review.

These documents are self-contained planning records. Temporary review artifacts are useful
background but must not be the only source of reproduction steps or acceptance requirements.

### Finding register

| Finding | Priority | Defect | Main implementation surfaces | Original task relationships |
| --- | --- | --- | --- | --- |
| F1 | P2 | Production state changes do not notify UI subscribers | `apps/daemon/src/{lib,runtime_health,control_worker,client_link}.rs` | P11-001/002; P12-002/007/010; P13-008 |
| F2 | P2 | An idle daemon's disappearance leaves event readers pending | `crates/blueroute-client/src/lib.rs`; desktop/TUI `watch.rs` | P5-005; P11-001; P12-002 |
| F3 | P2 | Replacing graph IDs leaves inconsistent segments and links | `crates/blueroute-core/src/topology.rs` | P2-005; P8/P9 prerequisites |
| F4 | P2 | Recovered hosting capability disagrees with the create precondition | daemon `single_star.rs`, `lib.rs`, and `runtime_health.rs` | P6-001; P12-004/005; P13-004 |
| F5 | P2 | Package removal lacks cleanup for generated network state | daemon lifecycle; Linux ownership adapters; `packaging/` | P15-005; P17-006 |
| F6 | P3 | Asynchronous effect subscriptions leak across StrictMode replay | `apps/desktop/ui/src/useDaemon.ts` | P12-002/012 |
| F7 | P3 | Current architecture, task summaries, and ADR inventory drift | `docs/SPEC.md`, `docs/TODO.md`, `docs/ADR-INDEX.md` | P17-003 |
| F8 | P2 | Version validation and a command can reach different service owners | `crates/blueroute-client/src/lib.rs` | P5-001/005; shared-client consumers |

P2 denotes medium-priority defects; P3 denotes lower-priority issues. Related original tasks
are traceability links, not permission to complete or reclassify their unrelated acceptance.

### Scope boundaries

- Keep production joining closed in ordinary builds; preserve the default-off `experimental-join`
  gate and the independently confirmed identity, pin, TLS, and exporter requirements.
- Keep Internet sharing disabled. Do not implement gateway, live routing, topology orchestration,
  GUI Join, or the rest of P1–P19 as incidental remediation work.
- Preserve the read-only TUI product decision. Its original action-workflow acceptance remains
  separately unresolved and must be described accurately.
- Retain existing component hardware evidence at its recorded scope and revision. New software
  evidence does not qualify Bluetooth, controller limits, multi-hop behavior, or a v1 release.
- These two new files do not themselves modify the existing specification, TODO, or code.

## 2. Shared invariants

1. The daemon remains authoritative. Frontends obtain snapshots through `blueroute-client`;
   they do not manipulate BlueZ, NetworkManager, routes, or privileges directly.
2. PolicyKit authorization remains exhaustive for mutations. Owner/version checks complement
   authorization and must not weaken it.
3. A normal command is delivered only to the unique daemon owner whose compatible version was
   validated for that operation. A lost reply never authorizes automatic mutation replay.
4. Events announce observable state after publication or commit. A notification is not evidence
   that an uncommitted operation succeeded, nor a substitute for the current snapshot.
5. Unknown, unsupported, stale, and observed-empty remain distinct. Control sessions are not
   evidence of PANU attachments, physical reachability, or a measured radio limit.
6. Mutations and cleanup require explicit ownership proof. Matching a name prefix, interface
   name, address, or Bluetooth device alone never grants ownership.
7. Failed graph operations leave the previous valid graph intact. Failed cleanup preserves
   enough evidence for a safe retry.
8. Core remains free of D-Bus/backend types; unsafe Rust remains forbidden; lint rules remain
   enabled. Production networking continues to use typed system APIs rather than shell tools.
9. Software regression tests must run without physical Bluetooth and must not alter live host
   services or networking. Privileged qualification uses a deliberately isolated test system.

## 3. F8: Bind version checks and operations to one daemon owner

### Required behavior

**OWN-1:** Resolve `org.blueroute.Service1` to its unique D-Bus owner. Perform compatibility
negotiation and the ensuing command against that same unique destination, including response
handling. Re-resolving the well-known name between the two operations is insufficient.

**OWN-2:** An owner disappearing before dispatch causes a typed unavailable/owner-change failure
or a fresh negotiation before dispatch. An incompatible replacement receives zero normal
requests. If the original owner relinquishes the name but remains connected, an operation may
finish against that already validated owner; it must never silently migrate to its successor.

**OWN-3:** Preserve the existing no-replay policy for mutations after a request may have been
delivered. Do not turn reconnect support into blind retry of create, leave, trust, or enrollment.
Existing compatibility rules and exact error-name classification remain intact.

**OWN-4:** Treat cached version information as owner-scoped. Subscription installation and event
validation must share the owner discipline described below. An event from a retired owner cannot
be accepted using the replacement owner's version result.

### Required regression evidence

Use a private broker with compatible owner A and replacement B. During A's request-time Version
handling, transfer the well-known name to B, which advertises incompatible API 99.0. Count B's
Request calls. The baseline incorrectly delivers CreateNetwork to B; the corrected implementation
must keep that count zero. Test compatible replacement, owner disappearance, queued old events,
and a lost reply without replay as separate cases. Use synchronization barriers, not timing luck.

## 4. F2: Observe service-owner loss independently of events

**WATCH-1:** Install owner-change observation without a missed-transition window: subscribe and
reconcile the current owner as one coherent setup sequence. Race owner lifecycle notifications
against event reads and bus closure. An idle owner loss or replacement must terminate/invalidate
the old subscription even when no application Event is ever emitted.

**WATCH-2:** Desktop and TUI watchers report a disconnected/unavailable link, then use bounded
reconnect backoff. Establish a fresh owner-bound, version-checked subscription and reload the
snapshot on compatible return. An incompatible owner stays unavailable; it cannot supply data
through the previous subscription. Unrelated bus-name changes do not disturb the connection.

**WATCH-3:** A snapshot retained for presentation during disconnection must be visibly stale or
unavailable. It must not continue presenting current healthy connectivity. Unmounted consumers
must stop receiving callbacks and release subscription resources.

**WATCH-4:** Healthy subscriptions do not poll Version in a busy loop. In a controlled local
integration test, owner loss must surface within two seconds without an Event. Reconnect uses
the existing one-second initial delay and 30-second cap unless a documented change is justified;
compatible return refreshes on the next scheduled attempt plus bounded connection work. Tests
may inject shorter clocks/delays while preserving the production schedule.

Required scenarios are idle stop, idle compatible restart, incompatible restart, direct A-to-B
replacement, bus disconnect, owner loss during subscription setup, old-owner queued events, and
consumer cancellation. Verify actual watcher transitions, not only delay arithmetic.

## 5. F1: Publish committed state changes to consumers

### Change coverage

| State change | Producer or boundary | Required observer result |
| --- | --- | --- |
| Create, join where enabled, leave | Successful network operation and public status update | Current network and available actions refresh |
| Rename | Durable name commit and public name update | Other connected frontends show the new label |
| Capability/system/health observation | Runtime observer | Prerequisites, diagnostics, and health refresh |
| Authenticated peer arrival/departure/revocation | Control worker's published observations | Device list and control-session count refresh |
| Client-link connecting/waiting/established/attention/give-up | Reconnector's published state | Client-link detail refreshes even when aggregate health is unchanged |
| Discovery/known-network or exposed diagnostic changes | Relevant published snapshot producer | Corresponding lists/details refresh |

**EVENT-1:** Introduce one explicit publication path usable by request dispatch and background
workers. Publish only after the authoritative in-memory snapshot is updated and any required
durable commit succeeds. Rollback must not advertise transient success; a failed operation that
changes exposed health/diagnostics may still notify that distinct change.

**EVENT-2:** Keep signal emission off locks and off blocking network/storage paths. Use bounded
transport or a coalesced dirty revision, rather than unbounded per-event work. If changes are
coalesced, the latest observable state must still trigger a refresh; saturation cannot silently
discard the final invalidation. Document ordering, duplicate, and coalescing semantics.

**EVENT-3:** Prefer semantically correct existing events where sufficient. If a general snapshot
invalidation signal or new variant is needed, document its version and compatibility contract
before adding it. Do not fabricate a health/topology event for an unrelated change or send a new
enum variant to older clients whose advertised compatibility would make decoding fail.

**EVENT-4:** A failed notification cannot convert a committed mutation into an operation failure
that encourages a user/client to repeat it. Record a payload-free diagnostic and retain pending
invalidation for bounded retry or fresh-subscription reconciliation. State remains authoritative.

**EVENT-5:** Subscribe before the initial read, or reread after subscription establishment, so
a change between first fetch and subscription cannot leave the initial view stale. On each new
compatible owner, reread snapshots rather than relying on replay of signals from the old owner.

**EVENT-6:** Once a transition is published in the daemon, a healthy connected subscriber must
receive invalidation within one second in the controlled local test environment. This bound
starts after observation/commit; it does not shorten the existing 15-second hardware probe
interval or claim radio-loss detection within one second.

Tests must change real DaemonService/worker state and observe the actual broker signal. They
must not call `emit_event` manually to simulate the behavior being tested. Cover unchanged
samples, failed/rolled-back requests, notifications after success, burst coalescing, emitter
failure, peer removal, and client-link changes with unchanged aggregate health. Frontends still
receive only their narrow typed snapshots and payload-free desktop change notifications.

## 6. F3: Make topology insertion and replacement consistent

**GRAPH-1:** Define and document the contract of `add_segment` and `add_link`, including their
boolean return values. Identical reinsertion is idempotent and cannot erase existing members.
Property updates for an existing link identity must not silently rebind that identity's endpoints
or segment without an explicit, validated policy.

**GRAPH-2:** Choose either rejection of incompatible identity rebinding or an atomic replacement
operation that updates all affected graph indexes. Rejection is the conservative default; if
callers require replacement, implement and test its full consistency contract before permitting it.

**GRAPH-3:** After every successful operation, link endpoints exist, each link references an
existing segment, and the link's NAP matches that segment's NAP. Membership bookkeeping includes
the NAP and all represented attached clients, without stale entries created by replacement.
Document whether explicitly supplied non-link members are supported; do not accidentally remove
such members or admit unknown nodes while repairing the observed-link bookkeeping.

**GRAPH-4:** Validate before mutation. Rejected segment-NAP replacement, link migration, or
invalid endpoint changes preserve the entire old graph. Removal keeps a client in a segment
while another represented link still requires it. Preserve deterministic snapshots and paths.

Tests cover same-segment reinsertion after clients attach, changed NAP, same LinkId with changed
PANU/NAP/segment, property-only update, invalid replacement, duplicate membership, multiple links
to one member, and removal after updates. Check graph membership and links together; supplement
with a downstream route/forwarding calculation to catch cross-model inconsistencies.

## 7. F4: Use current capability observations for hosting

**CAP-1:** Hosting decisions must not be permanently bound to the startup capability copy.
Reobserve prerequisites at the operation boundary or share a synchronized, versioned observation
with operational code. A published positive capability must no longer coexist indefinitely with
a stale negative gate for that same observation generation.

**CAP-2:** Capability freshness is explicit. After prerequisite recovery is observed, creation
can proceed without daemon restart, subject to current authorization, membership, address
conflicts, and actual backend success. Unknown, failed, expired, or explicitly unsupported
observations remain fail-closed. Recheck relevant runtime ownership before backend mutation.

**CAP-3:** Readiness shown in the UI describes observed prerequisites, not guaranteed admission.
A prerequisite lost between observation and mutation causes a typed operational refusal with
safe rollback and updated diagnostics. Reading capabilities alone must not create a network.

**CAP-4:** Updating capabilities cannot weaken peer identity checks, enroll peers, change durable
membership, clear a configured ceiling, or bypass the closed join gate. Audit other retained
capability copies, including control admission, for contradictory use of the refreshed data;
fix affected consumers or document deliberately latched policy separately from live capability.

Test false-to-true, unknown-to-true, true-to-false, and observation failure using the same service
and operations instance. Verify successful recovered creation reaches the fake runtime, unsafe
creation never does, published readiness agrees, and a concurrent generation change is handled.
Physical adapter-reset/multiple-radio qualification remains a separate acceptance item.

## 8. F5: Clean owned runtime state during package removal

### Lifecycle semantics

| Operation | Required behavior |
| --- | --- |
| Fresh installation | Start service without creating a network, enabling sharing, or enrolling peers |
| Daemon restart / package upgrade | Preserve durable identity/membership and intended restoration; never run remove/purge policy |
| Service stop | Document runtime resource behavior; a stop alone is not proof of removal cleanup |
| Package remove | Quiesce writers; remove verified installation-owned runtime networking; retain durable user state |
| Explicit purge | Complete safe cleanup before deleting the retained state/ownership evidence |
| Repeated removal cleanup | Succeed idempotently for already absent resources; preserve foreign state |

**CLEAN-1:** Select and document a daemon-owned cleanup entry point that works when the normal
service is stopped. A fixed maintenance mode/helper is acceptable. A call to the normal live
daemon alone is insufficient. Use typed Linux adapters; do not introduce `nmcli`, `bluetoothctl`,
or generated networking shell fragments into production cleanup.

**CLEAN-2:** Prevent runtime/reconnect/control workers from recreating state while cleanup runs.
Serialize cleanup with ongoing operations and startup restoration. Derive removal scope from
the current installation's verified ownership records and full NetworkIds, not names alone.
Define behavior for known networks, stale owned profiles, unknown orphaned records, and another
installation's resources. Ambiguous ownership is preserved and reported as unresolved.

**CLEAN-3:** Remove only proven owned PAN attachments, advertisements/NAP registrations, addresses,
routes, connections/profiles, and interfaces that the implementation currently creates. Retain
foreign profiles, foreign/default routes, non-owned addresses, and externally enabled forwarding.
Do not build future NAT/firewall features to satisfy cleanup of state the product never creates.

**CLEAN-4:** Release forwarding through its existing baseline/lease rules. Preserve the lease and
other ownership proof until cleanup no longer needs them: systemd RuntimeDirectory removal must
not erase `/run/blueroute` evidence before release. Validate ordering of PAN disconnection and
profile deletion so cleanup does not delete its own proof too early.

**CLEAN-5:** Cleanup is bounded and observation-based, and operates without treating Bluetooth
availability as a prerequisite for NetworkManager-only cleanup. If a backend is unavailable,
attempt independent safe work, report remaining resources, retain retry evidence, and return
failure rather than claiming success. Retries reobserve partial results instead of blindly
undoing foreign changes. Never clear membership merely to hide cleanup failure.

**CLEAN-6:** Maintainer scripts distinguish remove/deconfigure, upgrade, abort paths, and purge.
Before normal removal or purge can be called successful, cleanup must have succeeded. A cleanup
failure must not be hidden behind `|| true` or followed by deletion of the evidence needed for
retry. Validate the chosen failure behavior against Debian maintainer-script recovery paths;
explicit force-removal or manual administrator intervention is outside the success guarantee.

**CLEAN-7:** Log only bounded non-secret resource categories, outcomes, and coarse errors. The
entry point is administrator-only, accepts no arbitrary executable/backend command, and adds no
general-purpose frontend mutation or privilege escalation path.

Qualification has three distinct layers: deterministic fake-backend cleanup/failure tests;
isolated dpkg/helper execution and migration tests; then an isolated real NetworkManager/systemd
environment with an active owned bridge/address/profile and foreign comparison fixtures.
Physical PAN cleanup is additionally required for claims involving Bluetooth links. Neither
mock helper logs nor package file removal establish absence of active network state.

## 9. F6: Scope React resources to one effect generation

**UI-1:** Give each effect setup its own disposal/generation identity. A late subscription result
for a disposed setup must immediately unsubscribe exactly once, even after a later setup mounts.
Shared mounted state alone cannot identify the owner of an asynchronous resource.

**UI-2:** Ignore callbacks and fetch completions from disposed generations. They must not change
link/view state, start follow-up refreshes, replace the current refresh handler, or reset the new
generation's loading/coalescing state. Abort old reads when supported; otherwise discard them.

**UI-3:** Keep refresh coalescing bounded within the active generation. Mount/unmount, StrictMode
setup-cleanup-setup, and changing `api`/`subscribe` dependencies leave one live listener while
mounted and none after unmount. Keep StrictMode enabled and fix findings without lint exceptions.

Use deferred promises to test late subscribe completion, both possible resolution orders,
replacement APIs, stale fetch completion, events after cleanup, rejected subscription setup,
rapid unmount/remount, and multiple signals during one load. Assert both listener accounting and
visible data; convert the original leak demonstration into a correctness test that fails on the
baseline and passes only when all handles are released.

## 10. F7: Reconcile current documentation and acceptance status

**DOC-1:** Correct the current-state summaries against source and validated artifacts. At the
reviewed baseline the local API is 1.6, membership storage is schema v3, `blueroute` contains
daemon/CLI/TUI, and `blueroute-desktop` is a separate package. Recheck these facts at closeout;
remediation may legitimately change an API version or packaging detail.

**DOC-2:** Index the accepted P8-009 dynamic-routing ADR, distinguish design records from
accepted ADRs, and review the index against all existing accepted decisions. Reconcile stale
TLS/enrollment/runtime summaries without declaring gated outbound joining physically qualified.

**DOC-3:** Update current summaries in SPEC, the original TODO, threat model, support/hardware
tracker, and user/administrator/development/packaging documents where affected. Preserve stable
original task IDs and historical evidence; label superseded status prose instead of rewriting
past results as new evidence. Distinguish the real read-only TUI from obsolete placeholder notes.

**DOC-4:** Keep one clear current-status authority. Link this remediation backlog to original
tasks when implementation begins, without copying another competing P0–P19 status table. A
remediation fix does not close an original task whose physical or product acceptance remains.
Replace temporary artifact references in durable records with repository-maintained reproductions
or evidence summaries.

### Completion vocabulary

- **NOT STARTED:** no implementation evidence for the remediation task.
- **IN PROGRESS:** implementation, review, or required checks remain.
- **SOFTWARE COMPLETE / HARDWARE DEFERRED:** software criteria passed at an exact revision;
  required installed-system or physical criteria remain explicitly open.
- **COMPLETE:** every applicable acceptance criterion has evidence; no required qualification is
  merely assumed. Hardware-dependent closure identifies the tested environment and source SHA.

## 11. Integration and validation strategy

Tests must prove the corrected boundary behavior rather than mirror helper implementation:

| Scenario | Required observation | Findings |
| --- | --- | --- |
| Compatible A replaced by incompatible B between Version and Request | B receives zero normal requests; no mutation replay | F8 |
| Idle daemon disappears and a compatible owner returns | Disconnect/stale state, bounded reconnect, refreshed subscription/snapshot | F2, F8 |
| A second client renames or leaves; worker changes health/peers/client link | An already connected observer updates without manual refresh | F1, F2 |
| Effect replays while subscription/read promises are pending | No leaked listener, stale snapshot, or old-generation refresh | F6 |
| Reinsert/update/reject topology IDs | Graph indexes agree; rejection is atomic; routing remains deterministic | F3 |
| Same daemon starts incapable and later observes usable hosting | Creation proceeds using fresh prerequisites without restart | F4, F1 |
| Remove/purge with owned and foreign resources; repeat/fail/retry | Owned state converges to absent; foreign state and required evidence survive | F5 |
| Inspect current manifests/API/schema/ADRs against documentation | Current claims match validated code/artifacts and remaining gates | F7 |

Run affected focused checks during implementation, then the full existing gates at the final
revision. Maintain pinned toolchains and locked dependencies. Update CI to execute the new
real-broker/watcher/cleanup regressions; do not rely on an ignored test being exercised by the
ordinary workspace command.

```bash
cargo fmt --all -- --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p blueroute-daemon --features experimental-join --locked
python3 -m unittest discover -s scripts -p 'test_p6_006_*.py' -v
bash packaging/debian/test-maintainer-lifecycle.sh
```

Run existing and new ignored broker tests with `dbus-run-session`; retain the five existing
daemon, authorization, client, CLI-exit, and desktop-bridge CI checks. In `apps/desktop`, use the
pinned Node/npm, `npm ci`, then `npm run format:check`, `npm run lint`, `npm run typecheck`,
`npm test`, and `npm run build`.

Build both packages with `bash packaging/debian/build-package.sh`; build the current
`state_check` example and run the isolated upgrade test against that build's core package.
Exercise scripts embedded in the built packages. Use the standard system executable directories
in PATH. No host package installation/removal or radio mutation belongs in ordinary CI.

Record revision, commands, outcomes, regression coverage, package hashes, and any limitations.
Report actual new test counts instead of treating baseline counts as a fixed completion target.
For required hardware acceptance, also record machine/controller/firmware, distribution/kernel,
BlueZ/NetworkManager/systemd versions, exact procedure, observed state, and foreign-state checks.

## 12. Execution order and release gate

Recommended order: establish regressions; implement owner-bound operations (F8); repair owner
watching (F2); wire state publication (F1) and UI resource ownership (F6); fix graph consistency
(F3) and capability recovery (F4); implement/qualify package cleanup (F5); finish documentation
(F7); run combined integration and final qualification. Independent core/UI/documentation work
can proceed earlier without weakening these dependencies.

Close this remediation only when each code defect has a correctness regression, all eight
findings have reviewed fix evidence, cross-component tests run in CI, documentation is reconciled,
and all required cleanup/installed-system qualification is complete. If hardware remains deferred, record the
software milestone separately and leave full remediation and affected original gates open.
This remediation's completion does not by itself declare production join, routed networking,
heterogeneous support, or v1 ready.

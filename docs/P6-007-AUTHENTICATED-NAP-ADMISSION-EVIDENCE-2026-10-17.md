# P6-007 authenticated control-to-NAP admission seam (software only)

`apps/daemon/src/nap_control_admission.rs` adds typed reservation/activation APIs requiring a `RegisteredControlSession`, not a caller-asserted `NodeId`. Reservation checks network scope and revalidates durable approval on the registered TLS session. Activation revalidates approval again and binds the reservation to the exact TLS exporter session generation; a replaced authenticated session cannot activate a stale reservation. Teardown is generation/owner-scoped.

The daemon **must still** observe a real PANU attachment before calling activation, serialize session/link teardown against admission and install this seam into the live NAP listener lifecycle. The seam alone does not wire a listener, close a physical PAN socket, complete independent key enrollment UX, enable production JoinNetwork, or prove multi-client Bluetooth traffic. Original hardware acceptance remains deferred.

Qualification: the typed authenticated-session admission seam and deterministic generation/owner regressions passed exact-head CI at commit `9c62f803e910870b43f84eaa0d36138bace71298`, run [39078557539](https://github.com/ekkus93/blueroute/actions/runs/39078557539).

## RAII reservation lifecycle

`RegisteredControlSession::reserve_nap_lease` now returns an `AuthenticatedNapLease` scoped to the exact NAP owner. The lease retains the authenticated session generation, supports activation only through a current registered control session, and automatically retires its pending or active capacity on drop. An explicit retire is idempotent; stale callbacks cannot release a restarted owner's ticket. Pending/active lease-drop regressions passed [exact-head CI 39078928204](https://github.com/ekkus93/blueroute/actions/runs/39078928204) at `64b66dddfed565905aac6c251bed26721b363be6`.

The production daemon must retain the lease until the matching PAN attachment is actually gone, and wire cancellation to link loss, revocation, NAP shutdown and daemon lifecycle. This is still a software-only seam, not an observed Bluetooth attachment or production join.

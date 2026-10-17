# P6-007 authenticated control-to-NAP admission seam (software only)

`apps/daemon/src/nap_control_admission.rs` adds typed reservation/activation APIs requiring a `RegisteredControlSession`, not a caller-asserted `NodeId`. Reservation checks network scope and revalidates durable approval on the registered TLS session. Activation revalidates approval again and binds the reservation to the exact TLS exporter session generation; a replaced authenticated session cannot activate a stale reservation. Teardown is generation/owner-scoped.

The daemon **must still** observe a real PANU attachment before calling activation, serialize session/link teardown against admission and install this seam into the live NAP listener lifecycle. The seam alone does not wire a listener, close a physical PAN socket, complete independent key enrollment UX, enable production JoinNetwork, or prove multi-client Bluetooth traffic. Original hardware acceptance remains deferred.

Qualification: exact-head CI required for the source commit; record the SHA and run after validation.

# P9 topology policy software evidence — 2026-10-16

P9-001: `crates/blueroute-core/src/topology_policy.rs` now validates explicit heterogeneous capabilities, NAP capacity, candidate neighbors, link cost/quality/stability, current hosting state, power and gateway preferences. It rejects invalid identities, link metrics, battery percentages and zero NAP capacity.

P9-002: `select_nap_hubs` proposes capacity-bounded NAP stars only when NAP/PANU and adapter capability are affirmative and neighbor observations are reciprocal. It uses deterministic coverage and stability tie-breaks and reports unserved nodes instead of inventing connectivity. It does not operate BlueZ or authorize peers.

Implementation `b445703755e7e9faa49e8d708df259b1810957e0` passed CI 38990611759. Selection `3e32a480885b57e313cfcea228caad7be7e68636` passed CI 38990923557.

Still open: authenticated P7 transport, production join, P9 plan diff/executor, actual PAN measurements, and deferred physical acceptance. The authoritative large TODO has not been rewritten from partial content.

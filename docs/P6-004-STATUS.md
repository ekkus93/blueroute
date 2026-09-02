# P6-004 status

P6-004 is intentionally **blocked** after landing the fail-closed transactional join orchestration boundary.

The production `JoinNetwork` path must not be enabled until:

1. P6-005 provides conflict-aware one-star IPv4 address allocation/application; and
2. the required P7 work provides an authenticated control session that binds stable BlueRoute identity.

Until those prerequisites exist, `LinuxJoinRuntime::preflight` returns `CapabilityUnavailable` before Bluetooth or durable membership mutation. This is a security/correctness boundary, not a temporary success fallback.

See `docs/P6-004-JOIN-ORCHESTRATION.md` for the detailed dependency analysis, transactional ordering, rollback policy, and completion criteria.

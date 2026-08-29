# BlueRoute Persistence Schema Policy

BlueRoute durable state must evolve without requiring users to delete stable identity, remembered networks, peer membership, or trust state.

## Membership state

The membership-state filename remains `memberships-v1` for path compatibility with installations created by P3-003. The filename is not the schema-version authority. The first line inside the file is the authoritative schema header.

Current schema header:

```text
BLUEROUTE_MEMBERSHIP_V2
```

The P3-003 format is retained as schema version 1:

```text
BLUEROUTE_MEMBERSHIP_V1
```

## Migration contract

When loading membership state, BlueRoute:

1. detects the persisted schema version from the header;
2. rejects malformed version headers;
3. rejects schemas newer than the running daemon understands;
4. applies registered migrations one version at a time for supported older schemas;
5. validates the complete migrated document using the current-schema parser;
6. only after successful validation, atomically rewrites the state through the normal owner-only persistence path.

If a migration step is missing or migration/validation fails, BlueRoute returns a persistence error and leaves the original file unchanged. It does not silently reset durable membership or trust state.

## Current migration

The first registered migration is `V1 -> V2`. Version 2 establishes the migration framework while preserving the version-1 record semantics. This intentionally exercises a real backward-compatibility path using the format shipped by P3-003.

Future schema changes should add the next explicit migration step rather than changing an older migration in place. Tests should retain representative fixtures for each supported historical version and verify that stable identities and authorization-relevant state survive the complete migration chain.

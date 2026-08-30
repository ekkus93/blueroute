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

## Secret classification

As of P3-005, BlueRoute does not yet own a persisted cryptographic credential or shared secret.

Current durable values are classified as follows:

- `NodeId` is a stable public identifier. Its unpredictability prevents accidental collisions; it is not an authentication secret.
- remembered BlueRoute network membership and peer trust are authorization-sensitive state, but the identifiers and flags themselves are not cryptographic secrets. They remain owner-only because tampering or disclosure is still undesirable.
- Bluetooth pairing/link keys are owned and persisted by BlueZ. BlueRoute must not copy those keys into its own state files.
- future control-plane private keys, shared network keys, invitation/enrollment tokens, recovery material, or external-service credentials are secrets and must use the secret-handling boundary described below.

The classification must be updated when a new persistent field is introduced. A value must not be treated as non-secret merely because it is convenient to serialize with ordinary configuration or membership state.

## Secret handling boundary

Hardware-independent code wraps BlueRoute-owned secret values in `blueroute_core::Secret<T>`. Ordinary `Debug` and `Display` formatting of the wrapper emits only `[REDACTED]`; accessing the value requires an explicit `expose()` call. This is intended to prevent routine logging, debug dumps, and diagnostic formatting from leaking secret contents.

Linux file persistence for future BlueRoute-owned secret material uses `blueroute_linux::SecretFileStore` unless a later design deliberately selects a stronger platform-backed secret service. `SecretFileStore`:

- owns a dedicated secret directory and enforces mode `0700`;
- stores each secret in a regular file with mode `0600`;
- tightens overly broad existing directory/file permissions before reading;
- rejects symlink or other non-regular secret paths;
- validates secret names as a single path component so callers cannot escape the secret directory;
- writes through a same-directory temporary file, `fsync`s the file, atomically renames it, and `fsync`s the directory;
- never includes secret bytes in persistence error messages.

Secret material must not be written to `DaemonConfig`, membership records, command-line arguments, logs, or generic diagnostics merely to avoid using the secret boundary.

`Secret<T>` protects formatting boundaries; it is not a claim that arbitrary `T` is locked in RAM, immune to swapping, or cryptographically erased on drop. If future threat modeling requires memory locking or guaranteed zeroization, that must be implemented and tested explicitly rather than inferred from the redaction wrapper.

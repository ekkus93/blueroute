from pathlib import Path

path = Path("crates/blueroute-linux/src/membership_store.rs")
text = path.read_text()

old = '''/// The v1 format is deterministic and intentionally simple. Transient membership
/// states (`Joining` and `Leaving`) are rejected rather than persisted, because a
/// restart must recover a stable fact rather than resume an in-flight operation.
'''
new = '''/// Files carry explicit schema versions. Supported older schemas are migrated through
/// ordered steps, validated as the current schema, and atomically rewritten. Transient
/// membership states (`Joining` and `Leaving`) are rejected rather than persisted,
/// because a restart must recover a stable fact rather than resume an in-flight operation.
'''
if text.count(old) != 1:
    raise SystemExit("store documentation marker not found exactly once")
text = text.replace(old, new, 1)

marker = '''    #[test]
    fn future_schema_is_rejected_without_rewriting_state() {
'''
test = '''    #[test]
    fn missing_legacy_migration_path_is_rejected_without_rewriting_state() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let unsupported = "BLUEROUTE_MEMBERSHIP_V0\\n";
        fs::write(&path, unsupported).unwrap();

        let error = NetworkMembershipStore::new(&path).load().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PersistenceError);
        assert_eq!(fs::read_to_string(path).unwrap(), unsupported);
    }

'''
if text.count(marker) != 1:
    raise SystemExit("migration-path test marker not found exactly once")
text = text.replace(marker, test + marker, 1)

path.write_text(text)

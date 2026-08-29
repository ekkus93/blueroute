from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if text.count(old) != 1:
        raise SystemExit(f"{label}: expected one marker, found {text.count(old)}")
    return text.replace(old, new, 1)


path = Path("crates/blueroute-linux/src/membership_store.rs")
text = path.read_text()

text = replace_once(
    text,
    'const FORMAT_HEADER: &str = "BLUEROUTE_MEMBERSHIP_V1";\n',
    'const FORMAT_HEADER_PREFIX: &str = "BLUEROUTE_MEMBERSHIP_V";\n'
    'const LEGACY_FORMAT_VERSION: u32 = 1;\n'
    'const CURRENT_FORMAT_VERSION: u32 = 2;\n',
    "format constants",
)

old_load = '''        let serialized = fs::read_to_string(&self.path).map_err(|error| {
            persistence_error("failed to read the network membership state file", error)
        })?;
        parse_registry(&serialized)
'''
new_load = '''        let serialized = fs::read_to_string(&self.path).map_err(|error| {
            persistence_error("failed to read the network membership state file", error)
        })?;
        let migration = migrate_to_current(&serialized)?;
        let registry = parse_registry(&migration.serialized)?;
        if migration.migrated {
            self.save(&registry)?;
        }
        Ok(registry)
'''
text = replace_once(text, old_load, new_load, "load migration entry point")

migration_code = r'''#[derive(Debug, Eq, PartialEq)]
struct MigrationResult {
    serialized: String,
    migrated: bool,
}

fn migrate_to_current(serialized: &str) -> Result<MigrationResult, CoreError> {
    let original_version = detect_format_version(serialized)?;
    if original_version > CURRENT_FORMAT_VERSION {
        return Err(CoreError::new(
            ErrorKind::PersistenceError,
            format!(
                "network membership state uses unsupported future schema version {original_version}; current version is {CURRENT_FORMAT_VERSION}"
            ),
        ));
    }

    let mut version = original_version;
    let mut current = serialized.to_owned();
    while version < CURRENT_FORMAT_VERSION {
        current = match version {
            LEGACY_FORMAT_VERSION => migrate_v1_to_v2(&current)?,
            _ => {
                return Err(CoreError::new(
                    ErrorKind::PersistenceError,
                    format!(
                        "no network membership migration path exists from schema version {version}"
                    ),
                ));
            }
        };
        version += 1;
    }

    Ok(MigrationResult {
        serialized: current,
        migrated: original_version != CURRENT_FORMAT_VERSION,
    })
}

fn detect_format_version(serialized: &str) -> Result<u32, CoreError> {
    let header = serialized
        .lines()
        .next()
        .ok_or_else(|| malformed_state("missing membership format header"))?;
    let version = header
        .strip_prefix(FORMAT_HEADER_PREFIX)
        .ok_or_else(|| malformed_state("unsupported or missing membership format header"))?;
    if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(malformed_state("membership format version is invalid"));
    }

    version
        .parse::<u32>()
        .map_err(|error| malformed_value(1, "invalid membership format version", error))
}

fn migrate_v1_to_v2(serialized: &str) -> Result<String, CoreError> {
    let legacy_header = format_header(LEGACY_FORMAT_VERSION);
    let current_header = format_header(CURRENT_FORMAT_VERSION);
    let mut lines = serialized.lines();
    if lines.next() != Some(legacy_header.as_str()) {
        return Err(CoreError::new(
            ErrorKind::PersistenceError,
            "network membership v1 migration received the wrong source schema",
        ));
    }

    let mut migrated = String::new();
    migrated.push_str(&current_header);
    migrated.push('\n');
    for line in lines {
        migrated.push_str(line);
        migrated.push('\n');
    }

    // Validate the complete migrated document before any caller may rewrite disk state.
    parse_registry(&migrated)?;
    Ok(migrated)
}

fn format_header(version: u32) -> String {
    format!("{FORMAT_HEADER_PREFIX}{version}")
}

'''
serialize_marker = 'fn serialize_registry(registry: &MembershipRegistry) -> Result<String, CoreError> {\n'
text = replace_once(
    text,
    serialize_marker,
    migration_code + serialize_marker,
    "migration function insertion",
)
text = replace_once(
    text,
    '    let mut output = String::from(FORMAT_HEADER);\n',
    '    let mut output = format_header(CURRENT_FORMAT_VERSION);\n',
    "current-version serialization",
)

old_parse_header = '''    let mut lines = serialized.lines();
    if lines.next() != Some(FORMAT_HEADER) {
        return Err(malformed_state(
            "unsupported or missing membership format header",
        ));
    }
'''
new_parse_header = '''    let mut lines = serialized.lines();
    let current_header = format_header(CURRENT_FORMAT_VERSION);
    if lines.next() != Some(current_header.as_str()) {
        return Err(malformed_state(
            "unsupported or missing current membership format header",
        ));
    }
'''
text = replace_once(text, old_parse_header, new_parse_header, "current-version parser")

test_marker = '''    #[test]
    fn malformed_state_is_rejected_without_quiet_reset() {
'''
migration_tests = r'''    #[test]
    fn synthetic_v1_state_migrates_to_current_without_losing_membership() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let network_id = NetworkId::from_bytes([3; 16]);
        let member_peer = peer(4);
        let trusted_nonmember = peer(5);
        let legacy = format!(
            "BLUEROUTE_MEMBERSHIP_V1\nnetwork\t{network_id}\tmember\t4c6567616379204c6162\npeer\t{network_id}\t{member_peer}\t1\t1\npeer\t{network_id}\t{trusted_nonmember}\t0\t1\n"
        );
        fs::write(&path, legacy).unwrap();

        let store = NetworkMembershipStore::new(&path);
        let migrated = store.load().unwrap();
        let restored = migrated.network(&network_id).unwrap();
        assert_eq!(restored.network_name.as_str(), "Legacy Lab");
        assert_eq!(restored.state, MembershipState::Member);
        assert!(restored.is_peer_member(&member_peer));
        assert!(restored.is_peer_trusted(&member_peer));
        assert!(!restored.is_peer_member(&trusted_nonmember));
        assert!(restored.is_peer_trusted(&trusted_nonmember));

        let rewritten = fs::read_to_string(&path).unwrap();
        assert!(rewritten.starts_with("BLUEROUTE_MEMBERSHIP_V2\n"));
        assert!(!rewritten.contains("BLUEROUTE_MEMBERSHIP_V1"));
        assert_eq!(NetworkMembershipStore::new(&path).load().unwrap(), migrated);
    }

    #[test]
    fn save_uses_current_membership_schema_version() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let mut registry = MembershipRegistry::default();
        registry.remember_network(network(1, "Current", true));

        NetworkMembershipStore::new(&path).save(&registry).unwrap();
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .starts_with("BLUEROUTE_MEMBERSHIP_V2\n")
        );
    }

    #[test]
    fn future_schema_is_rejected_without_rewriting_state() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let future = "BLUEROUTE_MEMBERSHIP_V999\n";
        fs::write(&path, future).unwrap();

        let error = NetworkMembershipStore::new(&path).load().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PersistenceError);
        assert_eq!(fs::read_to_string(path).unwrap(), future);
    }

'''
text = replace_once(text, test_marker, migration_tests + test_marker, "migration tests")

path.write_text(text)

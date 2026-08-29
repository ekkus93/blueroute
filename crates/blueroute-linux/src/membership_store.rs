use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use blueroute_core::{
    CoreError, DisplayName, ErrorKind, MembershipRegistry, MembershipState, NetworkId,
    NetworkMembership, NodeId,
};

const FORMAT_HEADER_PREFIX: &str = "BLUEROUTE_MEMBERSHIP_V";
const LEGACY_FORMAT_VERSION: u32 = 1;
const CURRENT_FORMAT_VERSION: u32 = 2;
const MEMBERSHIP_FILE_MODE: u32 = 0o600;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

/// Durable file-backed registry of known BlueRoute networks and peer membership/trust facts.
///
/// The v1 format is deterministic and intentionally simple. Transient membership
/// states (`Joining` and `Leaving`) are rejected rather than persisted, because a
/// restart must recover a stable fact rather than resume an in-flight operation.
pub struct NetworkMembershipStore {
    path: PathBuf,
}

impl NetworkMembershipStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads all remembered networks. A missing file is an empty registry.
    pub fn load(&self) -> Result<MembershipRegistry, CoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == IoErrorKind::NotFound => {
                return Ok(MembershipRegistry::default());
            }
            Err(error) => {
                return Err(persistence_error(
                    "failed to inspect the network membership state file",
                    error,
                ));
            }
        };

        if !metadata.file_type().is_file() {
            return Err(CoreError::new(
                ErrorKind::PersistenceError,
                "network membership state path is not a regular file",
            ));
        }

        self.enforce_owner_only_permissions(metadata.permissions().mode())?;
        let serialized = fs::read_to_string(&self.path).map_err(|error| {
            persistence_error("failed to read the network membership state file", error)
        })?;
        let migration = migrate_to_current(&serialized)?;
        let registry = parse_registry(&migration.serialized)?;
        if migration.migrated {
            self.save(&registry)?;
        }
        Ok(registry)
    }

    /// Atomically replaces the durable registry with the supplied stable state.
    pub fn save(&self, registry: &MembershipRegistry) -> Result<(), CoreError> {
        let serialized = serialize_registry(registry)?;
        self.ensure_parent_directory()?;
        self.validate_destination()?;

        let temporary_path = self.temporary_path()?;
        let result = self.write_temporary_and_replace(&temporary_path, &serialized);
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    fn ensure_parent_directory(&self) -> Result<(), CoreError> {
        let parent = self.parent_directory();
        fs::create_dir_all(parent).map_err(|error| {
            persistence_error(
                "failed to create the network membership state directory",
                error,
            )
        })
    }

    fn validate_destination(&self) -> Result<(), CoreError> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_file() => Ok(()),
            Ok(_) => Err(CoreError::new(
                ErrorKind::PersistenceError,
                "network membership state path is not a regular file",
            )),
            Err(error) if error.kind() == IoErrorKind::NotFound => Ok(()),
            Err(error) => Err(persistence_error(
                "failed to inspect the network membership state file",
                error,
            )),
        }
    }

    fn temporary_path(&self) -> Result<PathBuf, CoreError> {
        let Some(file_name) = self.path.file_name() else {
            return Err(CoreError::new(
                ErrorKind::PersistenceError,
                "network membership state path must name a file",
            ));
        };
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let temporary_name = format!(
            ".{}.tmp-{}-{sequence}",
            file_name.to_string_lossy(),
            std::process::id()
        );
        Ok(self.parent_directory().join(temporary_name))
    }

    fn write_temporary_and_replace(
        &self,
        temporary_path: &Path,
        serialized: &str,
    ) -> Result<(), CoreError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(MEMBERSHIP_FILE_MODE)
            .open(temporary_path)
            .map_err(|error| {
                persistence_error(
                    "failed to create a temporary network membership state file",
                    error,
                )
            })?;

        file.write_all(serialized.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                persistence_error("failed to persist network membership state", error)
            })?;
        drop(file);

        fs::rename(temporary_path, &self.path).map_err(|error| {
            persistence_error("failed to replace the network membership state file", error)
        })?;
        sync_directory(self.parent_directory())
    }

    fn enforce_owner_only_permissions(&self, current_mode: u32) -> Result<(), CoreError> {
        if current_mode & 0o077 == 0 {
            return Ok(());
        }

        fs::set_permissions(&self.path, fs::Permissions::from_mode(MEMBERSHIP_FILE_MODE)).map_err(
            |error| persistence_error("failed to secure the network membership state file", error),
        )
    }

    fn parent_directory(&self) -> &Path {
        match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
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

fn serialize_registry(registry: &MembershipRegistry) -> Result<String, CoreError> {
    let mut output = format_header(CURRENT_FORMAT_VERSION);
    output.push('\n');

    for membership in registry.networks() {
        let state = durable_state_code(membership.state)?;
        let encoded_name = encode_hex(membership.network_name.as_str().as_bytes());
        output.push_str(&format!(
            "network\t{}\t{state}\t{encoded_name}\n",
            membership.network_id
        ));

        for peer in membership.peers() {
            output.push_str(&format!(
                "peer\t{}\t{}\t{}\t{}\n",
                membership.network_id,
                peer.node_id,
                bool_code(peer.is_member()),
                bool_code(peer.is_trusted())
            ));
        }
    }

    Ok(output)
}

fn parse_registry(serialized: &str) -> Result<MembershipRegistry, CoreError> {
    let mut lines = serialized.lines();
    let current_header = format_header(CURRENT_FORMAT_VERSION);
    if lines.next() != Some(current_header.as_str()) {
        return Err(malformed_state(
            "unsupported or missing current membership format header",
        ));
    }

    let mut registry = MembershipRegistry::default();
    for (offset, line) in lines.enumerate() {
        let line_number = offset + 2;
        if line.is_empty() {
            return Err(malformed_line(line_number, "blank records are not allowed"));
        }

        let mut fields = line.split('\t');
        let record_type = fields
            .next()
            .ok_or_else(|| malformed_line(line_number, "missing record type"))?;
        match record_type {
            "network" => parse_network_record(&mut registry, &mut fields, line_number)?,
            "peer" => parse_peer_record(&mut registry, &mut fields, line_number)?,
            _ => return Err(malformed_line(line_number, "unknown record type")),
        }
        if fields.next().is_some() {
            return Err(malformed_line(line_number, "record has too many fields"));
        }
    }

    Ok(registry)
}

fn parse_network_record<'a>(
    registry: &mut MembershipRegistry,
    fields: &mut impl Iterator<Item = &'a str>,
    line_number: usize,
) -> Result<(), CoreError> {
    let network_id = required_field(fields, line_number, "network id")?
        .parse::<NetworkId>()
        .map_err(|error| malformed_value(line_number, "invalid network id", error))?;
    if registry.network(&network_id).is_some() {
        return Err(malformed_line(line_number, "duplicate network record"));
    }

    let state = parse_durable_state(
        required_field(fields, line_number, "membership state")?,
        line_number,
    )?;
    let encoded_name = required_field(fields, line_number, "network name")?;
    let name_bytes = decode_hex(encoded_name, line_number)?;
    let name = String::from_utf8(name_bytes)
        .map_err(|error| malformed_value(line_number, "network name is not valid UTF-8", error))?;
    let display_name = DisplayName::new(name)
        .map_err(|error| malformed_value(line_number, "invalid network name", error))?;

    let mut membership = NetworkMembership::new(network_id, display_name);
    membership.state = state;
    registry.remember_network(membership);
    Ok(())
}

fn parse_peer_record<'a>(
    registry: &mut MembershipRegistry,
    fields: &mut impl Iterator<Item = &'a str>,
    line_number: usize,
) -> Result<(), CoreError> {
    let network_id = required_field(fields, line_number, "network id")?
        .parse::<NetworkId>()
        .map_err(|error| malformed_value(line_number, "invalid network id", error))?;
    let node_id = required_field(fields, line_number, "peer node id")?
        .parse::<NodeId>()
        .map_err(|error| malformed_value(line_number, "invalid peer node id", error))?;
    let member = parse_bool(
        required_field(fields, line_number, "peer member flag")?,
        line_number,
    )?;
    let trusted = parse_bool(
        required_field(fields, line_number, "peer trust flag")?,
        line_number,
    )?;

    let membership = registry.network_mut(&network_id).ok_or_else(|| {
        malformed_line(
            line_number,
            "peer record refers to a network that has not been declared",
        )
    })?;
    if membership.is_peer_known(&node_id) {
        return Err(malformed_line(line_number, "duplicate peer record"));
    }

    membership.remember_peer(node_id);
    membership.set_peer_member(node_id, member);
    if trusted {
        membership.trust_peer(node_id);
    }
    Ok(())
}

fn required_field<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    line_number: usize,
    field_name: &str,
) -> Result<&'a str, CoreError> {
    fields
        .next()
        .ok_or_else(|| malformed_line(line_number, &format!("missing {field_name}")))
}

fn durable_state_code(state: MembershipState) -> Result<&'static str, CoreError> {
    match state {
        MembershipState::NotMember => Ok("not-member"),
        MembershipState::Member => Ok("member"),
        MembershipState::Joining | MembershipState::Leaving => Err(CoreError::new(
            ErrorKind::PersistenceError,
            "cannot persist a transient network membership state",
        )),
    }
}

fn parse_durable_state(value: &str, line_number: usize) -> Result<MembershipState, CoreError> {
    match value {
        "not-member" => Ok(MembershipState::NotMember),
        "member" => Ok(MembershipState::Member),
        _ => Err(malformed_line(line_number, "invalid membership state")),
    }
}

const fn bool_code(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn parse_bool(value: &str, line_number: usize) -> Result<bool, CoreError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(malformed_line(line_number, "invalid boolean flag")),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_hex(value: &str, line_number: usize) -> Result<Vec<u8>, CoreError> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(malformed_line(line_number, "hex field has an odd length"));
    }

    let mut output = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = decode_nibble(pair[0])
            .ok_or_else(|| malformed_line(line_number, "hex field contains invalid data"))?;
        let low = decode_nibble(pair[1])
            .ok_or_else(|| malformed_line(line_number, "hex field contains invalid data"))?;
        output.push((high << 4) | low);
    }
    Ok(output)
}

const fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn sync_directory(path: &Path) -> Result<(), CoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| persistence_error("failed to sync the membership state directory", error))
}

fn persistence_error(message: &'static str, error: std::io::Error) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::PersistenceError, message, error.to_string())
}

fn malformed_state(message: &str) -> CoreError {
    CoreError::new(
        ErrorKind::PersistenceError,
        format!("network membership state is malformed: {message}"),
    )
}

fn malformed_line(line_number: usize, message: &str) -> CoreError {
    malformed_state(&format!("line {line_number}: {message}"))
}

fn malformed_value(line_number: usize, message: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::PersistenceError,
        format!("network membership state is malformed: line {line_number}: {message}"),
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_TEST_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "blueroute-membership-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn state_path(&self) -> PathBuf {
            self.0.join("memberships-v1")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn network(value: u8, name: &str, member: bool) -> NetworkMembership {
        let mut membership = NetworkMembership::new(
            NetworkId::from_bytes([value; 16]),
            DisplayName::new(name).unwrap(),
        );
        if member {
            membership.state = MembershipState::Member;
        }
        membership
    }

    fn peer(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    #[test]
    fn missing_state_file_loads_as_empty_registry() {
        let directory = TestDirectory::new();
        let store = NetworkMembershipStore::new(directory.state_path());
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn restart_preserves_known_network_and_peer_membership_and_trust() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let network_id = NetworkId::from_bytes([3; 16]);
        let member_peer = peer(4);
        let trusted_nonmember = peer(5);

        let mut membership = network(3, "Café Lab", true);
        membership.set_peer_member(member_peer, true);
        membership.trust_peer(member_peer);
        membership.remember_peer(trusted_nonmember);
        membership.trust_peer(trusted_nonmember);

        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);
        NetworkMembershipStore::new(&path).save(&registry).unwrap();

        let after_restart = NetworkMembershipStore::new(&path).load().unwrap();
        let restored = after_restart.network(&network_id).unwrap();
        assert_eq!(restored.network_name.as_str(), "Café Lab");
        assert_eq!(restored.state, MembershipState::Member);
        assert!(restored.is_peer_member(&member_peer));
        assert!(restored.is_peer_trusted(&member_peer));
        assert!(!restored.is_peer_member(&trusted_nonmember));
        assert!(restored.is_peer_trusted(&trusted_nonmember));
    }

    #[test]
    fn forget_peer_and_forget_network_survive_restart() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let kept_network = NetworkId::from_bytes([1; 16]);
        let forgotten_network = NetworkId::from_bytes([2; 16]);
        let forgotten_peer = peer(7);

        let mut kept = network(1, "Kept", true);
        kept.set_peer_member(forgotten_peer, true);
        kept.trust_peer(forgotten_peer);
        let mut registry = MembershipRegistry::default();
        registry.remember_network(kept);
        registry.remember_network(network(2, "Forgotten", false));

        let store = NetworkMembershipStore::new(&path);
        store.save(&registry).unwrap();
        let mut restored = store.load().unwrap();
        restored
            .network_mut(&kept_network)
            .unwrap()
            .forget_peer(&forgotten_peer);
        restored.forget_network(&forgotten_network);
        store.save(&restored).unwrap();

        let after_restart = NetworkMembershipStore::new(&path).load().unwrap();
        assert!(after_restart.network(&forgotten_network).is_none());
        assert!(
            !after_restart
                .network(&kept_network)
                .unwrap()
                .is_peer_known(&forgotten_peer)
        );
    }

    #[test]
    fn serialization_is_deterministic_across_insertion_order() {
        let first_directory = TestDirectory::new();
        let second_directory = TestDirectory::new();
        let first_path = first_directory.state_path();
        let second_path = second_directory.state_path();

        let mut low = network(1, "Low", true);
        low.trust_peer(peer(9));
        low.trust_peer(peer(2));
        let high = network(8, "High", false);

        let mut first = MembershipRegistry::default();
        first.remember_network(high.clone());
        first.remember_network(low.clone());
        let mut second = MembershipRegistry::default();
        second.remember_network(low);
        second.remember_network(high);

        NetworkMembershipStore::new(&first_path)
            .save(&first)
            .unwrap();
        NetworkMembershipStore::new(&second_path)
            .save(&second)
            .unwrap();
        assert_eq!(
            fs::read_to_string(first_path).unwrap(),
            fs::read_to_string(second_path).unwrap()
        );
    }

    #[test]
    fn transient_membership_state_is_not_persisted() {
        let directory = TestDirectory::new();
        let mut membership = network(1, "Joining", false);
        membership.state = MembershipState::Joining;
        let mut registry = MembershipRegistry::default();
        registry.remember_network(membership);

        let error = NetworkMembershipStore::new(directory.state_path())
            .save(&registry)
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PersistenceError);
        assert!(!directory.state_path().exists());
    }

    #[test]
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

    #[test]
    fn malformed_state_is_rejected_without_quiet_reset() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        fs::write(&path, "BLUEROUTE_MEMBERSHIP_V1\nnot-a-record\n").unwrap();

        let error = NetworkMembershipStore::new(&path).load().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PersistenceError);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "BLUEROUTE_MEMBERSHIP_V1\nnot-a-record\n"
        );
    }

    #[test]
    fn persisted_state_uses_owner_only_permissions() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        let mut registry = MembershipRegistry::default();
        registry.remember_network(network(1, "Private", true));

        let store = NetworkMembershipStore::new(&path);
        store.save(&registry).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, MEMBERSHIP_FILE_MODE);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        store.load().unwrap();
        let tightened = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(tightened, MEMBERSHIP_FILE_MODE);
    }

    #[test]
    fn non_regular_state_path_is_rejected() {
        let directory = TestDirectory::new();
        let path = directory.state_path();
        fs::create_dir(&path).unwrap();

        let error = NetworkMembershipStore::new(path).load().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PersistenceError);
    }
}

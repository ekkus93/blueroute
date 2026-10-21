use std::fs::{self, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use blueroute_core::{CoreError, ErrorKind};

const IPV4_FORWARD_SYSCTL: &str = "/proc/sys/net/ipv4/ip_forward";
const RUNTIME_DIR: &str = "/run/blueroute";
const LEASE_FILE: &str = "ipv4-forwarding-v1.state";
const LEASE_SCHEMA: &str = "1";
const LEASE_FILE_MODE: u32 = 0o600;

pub(crate) fn set_ipv4_forwarding(enabled: bool) -> Result<(), CoreError> {
    ForwardingController::system().set(enabled)
}

#[derive(Clone, Debug)]
struct ForwardingController {
    sysctl_path: PathBuf,
    lease_path: PathBuf,
}

impl ForwardingController {
    fn system() -> Self {
        Self {
            sysctl_path: PathBuf::from(IPV4_FORWARD_SYSCTL),
            lease_path: Path::new(RUNTIME_DIR).join(LEASE_FILE),
        }
    }

    #[cfg(test)]
    fn for_test(sysctl_path: PathBuf, lease_path: PathBuf) -> Self {
        Self {
            sysctl_path,
            lease_path,
        }
    }

    fn set(&self, enabled: bool) -> Result<(), CoreError> {
        if enabled {
            self.enable()
        } else {
            self.release()
        }
    }

    fn enable(&self) -> Result<(), CoreError> {
        let current = read_forwarding_value(&self.sysctl_path)?;
        let created = match read_lease(&self.lease_path)? {
            Some(_) => false,
            None => self.create_lease(current)?,
        };

        if current {
            return Ok(());
        }

        if let Err(write_error) = write_forwarding_value(&self.sysctl_path, true) {
            if created && let Err(cleanup_error) = remove_lease(&self.lease_path) {
                return Err(combined_rollback_error(&write_error, &cleanup_error));
            }
            return Err(write_error);
        }

        // Re-read the procfs control instead of trusting a successful write. This catches
        // restricted/containerized runtimes that accept the write without changing the node.
        match read_forwarding_value(&self.sysctl_path) {
            Ok(true) => Ok(()),
            Ok(false) => {
                if created {
                    remove_lease(&self.lease_path)?;
                }
                Err(CoreError::new(
                    ErrorKind::NetworkBackendUnavailable,
                    "kernel IPv4 forwarding did not become enabled after the requested write",
                ))
            }
            Err(error) => Err(error),
        }
    }

    fn release(&self) -> Result<(), CoreError> {
        let Some(lease) = read_lease(&self.lease_path)? else {
            // No BlueRoute lease means the current global value is foreign state. Never turn it
            // off just because a caller asked BlueRoute to release forwarding.
            return Ok(());
        };

        let current = read_forwarding_value(&self.sysctl_path)?;
        if current && !lease.baseline {
            write_forwarding_value(&self.sysctl_path, false)?;
            if read_forwarding_value(&self.sysctl_path)? {
                return Err(CoreError::new(
                    ErrorKind::NetworkBackendUnavailable,
                    "kernel IPv4 forwarding remained enabled while releasing the BlueRoute forwarding lease",
                ));
            }
        }
        // If forwarding was already changed externally while BlueRoute held the lease, do not
        // counteract that newer external decision. In particular, never write `1` during release.
        remove_lease(&self.lease_path)
    }

    /// Ensure a runtime lease exists and return whether this call created it.
    fn create_lease(&self, baseline: bool) -> Result<bool, CoreError> {
        let parent = self.lease_path.parent().ok_or_else(|| {
            CoreError::new(
                ErrorKind::Internal,
                "BlueRoute IPv4 forwarding lease path has no parent directory",
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            io_error(
                "failed to create the BlueRoute runtime directory for IPv4 forwarding",
                parent,
                error,
            )
        })?;

        let lease = ForwardingLease { baseline };
        let serialized = lease.serialize();
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(LEASE_FILE_MODE)
            .open(&self.lease_path)
        {
            Ok(mut file) => {
                let write_result = (|| {
                    file.write_all(serialized.as_bytes()).map_err(|error| {
                        io_error(
                            "failed to write the BlueRoute IPv4 forwarding lease",
                            &self.lease_path,
                            error,
                        )
                    })?;
                    file.sync_all().map_err(|error| {
                        io_error(
                            "failed to synchronize the BlueRoute IPv4 forwarding lease",
                            &self.lease_path,
                            error,
                        )
                    })?;
                    Ok::<(), CoreError>(())
                })();
                if let Err(write_error) = write_result {
                    if let Err(cleanup_error) = remove_lease(&self.lease_path) {
                        return Err(combined_rollback_error(&write_error, &cleanup_error));
                    }
                    return Err(write_error);
                }
                Ok(true)
            }
            Err(error) if error.kind() == IoErrorKind::AlreadyExists => {
                // A concurrent BlueRoute caller may have created the lease after our first read.
                // Parse it before proceeding; never overwrite or silently accept corrupt state.
                read_lease(&self.lease_path)?.ok_or_else(|| {
                    CoreError::new(
                        ErrorKind::InvalidState,
                        "BlueRoute IPv4 forwarding lease disappeared during concurrent creation",
                    )
                })?;
                Ok(false)
            }
            Err(error) => Err(io_error(
                "failed to create the BlueRoute IPv4 forwarding lease",
                &self.lease_path,
                error,
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ForwardingLease {
    baseline: bool,
}

impl ForwardingLease {
    fn serialize(self) -> String {
        format!(
            "schema={LEASE_SCHEMA}\nbaseline={}\n",
            if self.baseline { 1 } else { 0 }
        )
    }

    fn parse(contents: &str) -> Result<Self, CoreError> {
        let mut lines = contents.lines();
        let schema = lines.next();
        let baseline = lines.next();
        if schema != Some("schema=1") || lines.next().is_some() {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "BlueRoute IPv4 forwarding lease is malformed or uses an unsupported schema",
                contents.to_owned(),
            ));
        }
        let baseline = match baseline {
            Some("baseline=0") => false,
            Some("baseline=1") => true,
            _ => {
                return Err(CoreError::with_diagnostic(
                    ErrorKind::InvalidState,
                    "BlueRoute IPv4 forwarding lease has an invalid baseline value",
                    contents.to_owned(),
                ));
            }
        };
        Ok(Self { baseline })
    }
}

fn read_forwarding_value(path: &Path) -> Result<bool, CoreError> {
    let value = fs::read_to_string(path)
        .map_err(|error| io_error("failed to read kernel IPv4 forwarding state", path, error))?;
    match value.trim() {
        "0" => Ok(false),
        "1" => Ok(true),
        other => Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "kernel IPv4 forwarding state is not 0 or 1",
            format!("path={} value={other:?}", path.display()),
        )),
    }
}

fn write_forwarding_value(path: &Path, enabled: bool) -> Result<(), CoreError> {
    fs::write(path, if enabled { "1\n" } else { "0\n" }).map_err(|error| {
        io_error(
            if enabled {
                "failed to enable kernel IPv4 forwarding"
            } else {
                "failed to restore kernel IPv4 forwarding state"
            },
            path,
            error,
        )
    })
}

fn read_lease(path: &Path) -> Result<Option<ForwardingLease>, CoreError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "BlueRoute IPv4 forwarding lease is not a regular file",
            ));
        }
        Err(error) if error.kind() == IoErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error(
                "failed to inspect the BlueRoute IPv4 forwarding lease",
                path,
                error,
            ));
        }
    };
    if metadata.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(LEASE_FILE_MODE)).map_err(
            |error| {
                io_error(
                    "failed to secure the BlueRoute IPv4 forwarding lease",
                    path,
                    error,
                )
            },
        )?;
    }
    match fs::read_to_string(path) {
        Ok(contents) => ForwardingLease::parse(&contents).map(Some),
        Err(error) if error.kind() == IoErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(
            "failed to read the BlueRoute IPv4 forwarding lease",
            path,
            error,
        )),
    }
}

fn remove_lease(path: &Path) -> Result<(), CoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == IoErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(
            "failed to remove the BlueRoute IPv4 forwarding lease",
            path,
            error,
        )),
    }
}

fn io_error(message: &str, path: &Path, error: std::io::Error) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::NetworkBackendUnavailable,
        message,
        format!("path={} error={error}", path.display()),
    )
}

fn combined_rollback_error(primary: &CoreError, cleanup: &CoreError) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::NetworkBackendUnavailable,
        "IPv4 forwarding operation failed and BlueRoute could not roll back its forwarding lease",
        format!(
            "primary-error={}; lease-cleanup-error={}",
            diagnostic(primary),
            diagnostic(cleanup)
        ),
    )
}

fn diagnostic(error: &CoreError) -> String {
    error
        .diagnostic()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    use blueroute_core::{
        CapabilitySource, LinkHealth, LinkId, LinkState, NodeCapabilities, NodeId, PanLink,
        PanSegment, SegmentId, Sourced, TopologyGraph,
    };
    use futures_lite::future::block_on;

    use super::*;
    use crate::{
        BackendFuture, InterfaceAddress, IpNetworkBackend, LinuxRoute, reconcile_ipv4_forwarding,
    };

    struct TestPaths {
        root: PathBuf,
        sysctl: PathBuf,
        lease: PathBuf,
    }

    impl TestPaths {
        fn new(initial: bool) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "blueroute-forwarding-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            let sysctl = root.join("ip_forward");
            let lease = root.join("run/blueroute/ipv4-forwarding-v1.state");
            fs::write(&sysctl, if initial { "1\n" } else { "0\n" }).unwrap();
            Self {
                root,
                sysctl,
                lease,
            }
        }

        fn controller(&self) -> ForwardingController {
            ForwardingController::for_test(self.sysctl.clone(), self.lease.clone())
        }
    }

    impl Drop for TestPaths {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    // Exercise the actual file-backed Linux forwarding lease through the
    // topology reconciliation entry point, not just a mock boolean backend.
    struct LeaseBackend {
        controller: ForwardingController,
    }

    impl IpNetworkBackend for LeaseBackend {
        fn addresses(&self) -> BackendFuture<'_, Vec<InterfaceAddress>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn ensure_address(&self, _address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn remove_address(&self, _address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn routes(&self) -> BackendFuture<'_, Vec<LinuxRoute>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn ensure_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn remove_route(&self, _route: LinuxRoute) -> BackendFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn set_ipv4_forwarding(&self, enabled: bool) -> BackendFuture<'_, ()> {
            Box::pin(async move { self.controller.set(enabled) })
        }
    }

    fn router_node(value: u8) -> NodeId {
        NodeId::from_bytes([value; 16])
    }

    fn router_graph() -> TopologyGraph {
        let mut graph = TopologyGraph::default();
        for value in 1..=3 {
            graph.add_node(
                router_node(value),
                NodeCapabilities {
                    routing: if value == 2 {
                        Some(Sourced::new(true, CapabilitySource::Configured))
                    } else {
                        None
                    },
                    ..NodeCapabilities::default()
                },
            );
        }
        for (segment, nap) in [(1, 1), (2, 2)] {
            graph
                .add_segment(PanSegment::new(
                    SegmentId::from_bytes([segment; 16]),
                    router_node(nap),
                ))
                .unwrap();
        }
        for (id, segment, nap, panu) in [(1, 1, 1, 2), (2, 2, 2, 3)] {
            graph
                .add_link(PanLink {
                    id: LinkId::from_bytes([id; 16]),
                    segment_id: SegmentId::from_bytes([segment; 16]),
                    nap: router_node(nap),
                    panu: router_node(panu),
                    state: LinkState::Active,
                    health: LinkHealth::Healthy,
                    cost: 1,
                })
                .unwrap();
        }
        graph
    }

    #[test]
    fn topology_reconciliation_preserves_lease_across_restart_and_network_loss() {
        let paths = TestPaths::new(false);
        let active = router_graph();
        let mut inactive = active.clone();
        inactive.remove_link(&LinkId::from_bytes([2; 16]));
        let backend = LeaseBackend {
            controller: paths.controller(),
        };

        assert!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                &[active.clone(), inactive.clone()],
                router_node(2),
            ))
            .unwrap()
        );
        assert!(read_forwarding_value(&paths.sysctl).unwrap());
        assert_eq!(
            read_lease(&paths.lease).unwrap(),
            Some(ForwardingLease { baseline: false })
        );

        // An incomplete observation must not release an existing host-wide lease.
        assert_eq!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                &[inactive.clone(), TopologyGraph::default()],
                router_node(2),
            ))
            .unwrap_err()
            .kind(),
            ErrorKind::InvalidInput
        );
        assert!(read_forwarding_value(&paths.sysctl).unwrap());
        assert!(paths.lease.exists());

        // Simulate an external sysctl reset and a fresh backend after daemon restart.
        fs::write(&paths.sysctl, "0\n").unwrap();
        let restarted = LeaseBackend {
            controller: paths.controller(),
        };
        assert!(
            block_on(reconcile_ipv4_forwarding(
                &restarted,
                &[inactive.clone(), active],
                router_node(2),
            ))
            .unwrap()
        );
        assert!(read_forwarding_value(&paths.sysctl).unwrap());

        // Losing the last routing network releases only the BlueRoute lease.
        assert!(
            !block_on(reconcile_ipv4_forwarding(
                &restarted,
                &[inactive],
                router_node(2),
            ))
            .unwrap()
        );
        assert!(!read_forwarding_value(&paths.sysctl).unwrap());
        assert!(!paths.lease.exists());
    }

    #[test]
    fn topology_reconciliation_preserves_foreign_forwarding_baseline() {
        let paths = TestPaths::new(true);
        let backend = LeaseBackend {
            controller: paths.controller(),
        };
        assert!(
            block_on(reconcile_ipv4_forwarding(
                &backend,
                &[router_graph()],
                router_node(2),
            ))
            .unwrap()
        );
        assert_eq!(
            read_lease(&paths.lease).unwrap(),
            Some(ForwardingLease { baseline: true })
        );
        assert!(!block_on(reconcile_ipv4_forwarding(&backend, &[], router_node(2),)).unwrap());
        assert!(read_forwarding_value(&paths.sysctl).unwrap());
        assert!(!paths.lease.exists());
    }

    #[test]
    fn enable_and_release_restores_disabled_baseline_idempotently() {
        let paths = TestPaths::new(false);
        let controller = paths.controller();

        controller.set(true).unwrap();
        controller.set(true).unwrap();
        assert!(read_forwarding_value(&paths.sysctl).unwrap());
        assert_eq!(
            read_lease(&paths.lease).unwrap(),
            Some(ForwardingLease { baseline: false })
        );

        controller.set(false).unwrap();
        controller.set(false).unwrap();
        assert!(!read_forwarding_value(&paths.sysctl).unwrap());
        assert_eq!(read_lease(&paths.lease).unwrap(), None);
    }

    #[test]
    fn preexisting_foreign_forwarding_is_preserved_after_release() {
        let paths = TestPaths::new(true);
        let controller = paths.controller();

        controller.set(true).unwrap();
        assert_eq!(
            read_lease(&paths.lease).unwrap(),
            Some(ForwardingLease { baseline: true })
        );
        controller.set(false).unwrap();
        assert!(read_forwarding_value(&paths.sysctl).unwrap());
        assert_eq!(read_lease(&paths.lease).unwrap(), None);
    }

    #[test]
    fn disable_without_blueroute_lease_does_not_touch_foreign_state() {
        let paths = TestPaths::new(true);
        paths.controller().set(false).unwrap();
        assert!(read_forwarding_value(&paths.sysctl).unwrap());
        assert!(!paths.lease.exists());
    }

    #[test]
    fn fresh_controller_recovers_runtime_lease() {
        let paths = TestPaths::new(false);
        paths.controller().set(true).unwrap();
        let reconnected = paths.controller();
        reconnected.set(true).unwrap();
        assert!(read_forwarding_value(&paths.sysctl).unwrap());
        reconnected.set(false).unwrap();
        assert!(!read_forwarding_value(&paths.sysctl).unwrap());
        assert!(!paths.lease.exists());
    }

    #[test]
    fn release_never_reenables_forwarding_after_external_disable() {
        let paths = TestPaths::new(true);
        let controller = paths.controller();
        controller.set(true).unwrap();
        fs::write(&paths.sysctl, "0\n").unwrap();

        controller.set(false).unwrap();
        assert!(!read_forwarding_value(&paths.sysctl).unwrap());
        assert!(!paths.lease.exists());
    }

    #[test]
    fn malformed_lease_fails_closed_without_mutating_kernel_state() {
        let paths = TestPaths::new(false);
        fs::create_dir_all(paths.lease.parent().unwrap()).unwrap();
        fs::write(&paths.lease, "schema=99\nbaseline=0\n").unwrap();

        let error = paths.controller().set(true).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert!(!read_forwarding_value(&paths.sysctl).unwrap());
    }

    #[test]
    fn symlink_lease_fails_closed_without_reading_target_or_changing_forwarding() {
        let paths = TestPaths::new(false);
        fs::create_dir_all(paths.lease.parent().unwrap()).unwrap();
        let target = paths.root.join("outside");
        fs::write(&target, "schema=1\nbaseline=0\n").unwrap();
        symlink(&target, &paths.lease).unwrap();

        let error = paths.controller().set(false).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert!(!read_forwarding_value(&paths.sysctl).unwrap());
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            "schema=1\nbaseline=0\n"
        );
        assert!(
            fs::symlink_metadata(&paths.lease)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn existing_forwarding_lease_permissions_are_tightened_before_use() {
        let paths = TestPaths::new(false);
        let controller = paths.controller();
        controller.set(true).unwrap();
        fs::set_permissions(&paths.lease, fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(
            read_lease(&paths.lease).unwrap(),
            Some(ForwardingLease { baseline: false })
        );
        assert_eq!(
            fs::metadata(&paths.lease).unwrap().permissions().mode() & 0o777,
            LEASE_FILE_MODE
        );
        controller.set(false).unwrap();
        assert!(!read_forwarding_value(&paths.sysctl).unwrap());
    }

    #[test]
    fn invalid_kernel_value_fails_closed() {
        let paths = TestPaths::new(false);
        fs::write(&paths.sysctl, "unexpected\n").unwrap();
        let error = paths.controller().set(true).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert!(!paths.lease.exists());
    }
}

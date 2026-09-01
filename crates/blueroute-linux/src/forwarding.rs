use std::fs::{self, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use blueroute_core::{CoreError, ErrorKind};

const IPV4_FORWARD_SYSCTL: &str = "/proc/sys/net/ipv4/ip_forward";
const RUNTIME_DIR: &str = "/run/blueroute";
const LEASE_FILE: &str = "ipv4-forwarding-v1.state";
const LEASE_SCHEMA: &str = "1";
const LEASE_FILE_MODE: u32 = 0o600;

pub(crate) fn set_ipv4_forwarding(enabled: bool) -> Result<(), CoreError> {
    let controller = ForwardingController::system();
    controller.set(enabled)
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
        let (lease, created) = match read_lease(&self.lease_path)? {
            Some(lease) => (lease, false),
            None => (self.create_lease(current)?, true),
        };

        if current {
            return Ok(());
        }

        if let Err(write_error) = write_forwarding_value(&self.sysctl_path, true) {
            if created {
                if let Err(cleanup_error) = remove_lease(&self.lease_path) {
                    return Err(CoreError::with_diagnostic(
                        ErrorKind::NetworkBackendUnavailable,
                        "failed to enable IPv4 forwarding and could not roll back the BlueRoute forwarding lease",
                        format!(
                            "write-error={}; lease-cleanup-error={}",
                            diagnostic(&write_error),
                            diagnostic(&cleanup_error)
                        ),
                    ));
                }
            }
            return Err(write_error);
        }

        // Re-read the procfs control instead of trusting a successful write. This catches
        // restricted/containerized runtimes that accept the write without changing the node.
        if !read_forwarding_value(&self.sysctl_path)? {
            if created {
                remove_lease(&self.lease_path)?;
            }
            return Err(CoreError::new(
                ErrorKind::NetworkBackendUnavailable,
                "kernel IPv4 forwarding did not become enabled after the requested write",
            ));
        }

        // Touch the value so a corrupt/unsupported future lease cannot be silently accepted.
        let _ = lease.baseline;
        Ok(())
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

    fn create_lease(&self, baseline: bool) -> Result<ForwardingLease, CoreError> {
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
                Ok(lease)
            }
            Err(error) if error.kind() == IoErrorKind::AlreadyExists => {
                read_lease(&self.lease_path)?.ok_or_else(|| {
                    CoreError::new(
                        ErrorKind::InvalidState,
                        "BlueRoute IPv4 forwarding lease disappeared during concurrent creation",
                    )
                })
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

fn diagnostic(error: &CoreError) -> String {
    error
        .diagnostic()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

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
    fn invalid_kernel_value_fails_closed() {
        let paths = TestPaths::new(false);
        fs::write(&paths.sysctl, "unexpected\n").unwrap();
        let error = paths.controller().set(true).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert!(!paths.lease.exists());
    }
}

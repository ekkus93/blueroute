//! Cleanup of this installation's networking when the package is removed (CR-F5).
//!
//! The design, scope and Debian behavior are in `docs/CR-F5-REMOVAL-CLEANUP.md`. In short: the
//! daemon binary's `--remove-owned-networking` maintenance mode runs while the service is stopped,
//! takes the daemon's instance lock so no daemon can run beside it, leaves a marker that keeps any
//! daemon from starting again, and removes only NetworkManager state (and a proven-owned PANU
//! link) carrying the ownership of a network this installation knows. It then reobserves and
//! succeeds only if none of that state remains.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind, NetworkId};
use blueroute_linux::{BackendFuture, NetworkMembershipStore, PeerHandle};
use futures_lite::future;

use crate::client_leave::{ClientLeaveBackend, leave_observed};
use crate::client_record::ClientRecordStore;

/// Held by a running daemon for its whole life, and by cleanup while it runs.
pub const DAEMON_LOCK_FILE: &str = "daemon.lock";
/// While this exists in the state directory, no daemon starts (removal is in progress or done).
pub const REMOVAL_PENDING_FILE: &str = "removal-pending";
/// The whole cleanup must finish within this bound, or it fails.
pub const CLEANUP_BOUND: Duration = Duration::from_secs(60);

/// What kind of owned NetworkManager resource an inventory entry is.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum OwnedKind {
    Address,
    Profile,
}

/// The cleanup backend: leave operations plus an inventory of everything carrying BlueRoute
/// ownership, so the result can be reobserved rather than assumed.
pub trait OwnedInventory: ClientLeaveBackend {
    /// Every NetworkManager address and profile carrying BlueRoute ownership, with its owner.
    fn owned_resources(&self) -> BackendFuture<'_, Vec<(NetworkId, OwnedKind)>>;
}

/// The outcome of one cleanup. Counts only: nothing here names a resource or carries an error
/// message, so it is safe to log.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RemovalReport {
    /// Networks this installation knows, and so cleaned.
    pub networks: usize,
    /// Of those, how many had a removal step fail.
    pub failed_networks: usize,
    /// Owned resources of known networks still present afterwards.
    pub remaining: usize,
    /// Resources carrying BlueRoute ownership of networks this installation does not know.
    /// Preserved, and reported, but not a failure: they are not this installation's.
    pub unresolved: usize,
    /// Whether Bluetooth was reachable, so a proven-owned PANU link could be disconnected.
    pub bluetooth: bool,
}

impl RemovalReport {
    pub fn succeeded(&self) -> bool {
        self.failed_networks == 0 && self.remaining == 0
    }
}

/// Where the cleanup finds this installation's records.
pub struct RemovalPaths {
    pub state_directory: PathBuf,
    pub memberships: PathBuf,
    pub client_record: PathBuf,
}

/// An exclusive hold on the state directory's instance lock, released when dropped.
pub struct InstanceLock {
    _file: File,
}

/// Take the instance lock without waiting. Fails while a daemon or another cleanup holds it.
pub fn acquire_instance_lock(state_directory: &Path) -> Result<InstanceLock, CoreError> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state_directory)
        .map_err(|error| io_error("BlueRoute state directory is unavailable", &error))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(state_directory.join(DAEMON_LOCK_FILE))
        .map_err(|error| io_error("BlueRoute instance lock is unavailable", &error))?;
    match file.try_lock() {
        Ok(()) => Ok(InstanceLock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(CoreError::new(
            ErrorKind::InvalidState,
            "a BlueRoute daemon or cleanup is running; stop blueroute.service first",
        )),
        Err(TryLockError::Error(error)) => {
            Err(io_error("BlueRoute instance lock is unavailable", &error))
        }
    }
}

/// Whether a removal has started; a daemon must not start while this is true.
pub fn removal_pending(state_directory: &Path) -> bool {
    state_directory.join(REMOVAL_PENDING_FILE).exists()
}

fn mark_removal_pending(state_directory: &Path) -> Result<(), CoreError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(state_directory.join(REMOVAL_PENDING_FILE))
        .map_err(|error| io_error("BlueRoute removal marker could not be written", &error))?;
    file.write_all(b"package removal in progress; no daemon may start\n")
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error("BlueRoute removal marker could not be written", &error))
}

/// The networks this installation owns state for, each with the recorded host device of a joined
/// client (an untrusted hint, used only to find a PANU link to prove).
pub fn removal_scope(
    paths: &RemovalPaths,
) -> Result<BTreeMap<NetworkId, Option<PeerHandle>>, CoreError> {
    // An unreadable registry fails: ownership could not be proven.
    let registry = NetworkMembershipStore::new(paths.memberships.clone()).load()?;
    let mut known: BTreeMap<NetworkId, Option<PeerHandle>> = registry
        .networks()
        .map(|membership| (membership.network_id, None))
        .collect();
    if let Some(record) = ClientRecordStore::new(paths.client_record.clone()).load()? {
        known.insert(record.network(), record.device_hint().cloned());
    }
    Ok(known)
}

/// Remove every known network's owned state, then reobserve. One network's failure does not
/// stop the others; nothing without a known network's ownership is touched.
pub async fn remove_known_networks(
    backend: &dyn OwnedInventory,
    known: &BTreeMap<NetworkId, Option<PeerHandle>>,
    bluetooth: bool,
) -> Result<RemovalReport, CoreError> {
    let mut report = RemovalReport {
        networks: known.len(),
        bluetooth,
        ..RemovalReport::default()
    };
    for (network, hint) in known {
        // Without Bluetooth there is no PANU link to look at; NetworkManager cleanup still runs.
        let hint = if bluetooth { hint.as_ref() } else { None };
        if leave_observed(backend, *network, None, hint).await.is_err() {
            report.failed_networks += 1;
        }
    }
    for (owner, _) in backend.owned_resources().await? {
        if known.contains_key(&owner) {
            report.remaining += 1;
        } else {
            report.unresolved += 1;
        }
    }
    Ok(report)
}

/// The whole maintenance run, with the backend connection injected: lock, marker, scope, then
/// bounded cleanup. `connect` returns the backend and whether Bluetooth is reachable.
pub async fn run_removal_cleanup<B, Connect, Connecting>(
    paths: &RemovalPaths,
    connect: Connect,
    bound: Duration,
) -> Result<RemovalReport, CoreError>
where
    B: OwnedInventory,
    Connect: FnOnce() -> Connecting,
    Connecting: Future<Output = Result<(B, bool), CoreError>>,
{
    let _lock = acquire_instance_lock(&paths.state_directory)?;
    mark_removal_pending(&paths.state_directory)?;
    let known = removal_scope(paths)?;
    if known.is_empty() {
        // This installation knows no network, so it can own no networking state.
        return Ok(RemovalReport::default());
    }
    future::or(
        async {
            let (backend, bluetooth) = connect().await?;
            remove_known_networks(&backend, &known, bluetooth).await
        },
        async {
            Timer::after(bound).await;
            Err(CoreError::new(
                ErrorKind::NetworkBackendUnavailable,
                "BlueRoute removal cleanup did not finish in time",
            ))
        },
    )
    .await
}

fn io_error(message: &'static str, error: &std::io::Error) -> CoreError {
    CoreError::with_diagnostic(ErrorKind::Internal, message, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use blueroute_core::{DisplayName, IpPrefix, MembershipRegistry, NetworkMembership};
    use blueroute_linux::{InterfaceAddress, NetworkInterfaceHandle, PanAttachment, PanRole};

    use super::*;
    use crate::client_record::ClientRecord;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn net(byte: u8) -> NetworkId {
        NetworkId::from_bytes([byte; 16])
    }

    fn iface(name: &str) -> NetworkInterfaceHandle {
        NetworkInterfaceHandle::new(name).unwrap()
    }

    fn host_device() -> PeerHandle {
        PeerHandle::new("/org/bluez/hci0/dev_AA_00_00_00_00_01").unwrap()
    }

    fn address(owner: NetworkId, name: &str) -> InterfaceAddress {
        InterfaceAddress {
            interface: iface(name),
            prefix: IpPrefix::new("192.168.77.1".parse().unwrap(), 24).unwrap(),
            owner,
        }
    }

    /// NetworkManager and BlueZ as plain data. `unremovable` profiles survive removal.
    #[derive(Default)]
    struct World {
        addresses: Vec<InterfaceAddress>,
        profiles: Vec<(NetworkId, NetworkInterfaceHandle)>,
        unremovable: Vec<NetworkInterfaceHandle>,
        panu: Option<PanAttachment>,
        disconnected: usize,
        fail_inventory: bool,
    }

    #[derive(Default)]
    struct Fake(Mutex<World>);

    impl Fake {
        fn with(build: impl FnOnce(&mut World)) -> Self {
            let fake = Self::default();
            build(&mut fake.0.lock().unwrap());
            fake
        }

        fn world<T>(&self, read: impl FnOnce(&World) -> T) -> T {
            read(&self.0.lock().unwrap())
        }
    }

    impl ClientLeaveBackend for Fake {
        fn owned_addresses(&self, _network: NetworkId) -> BackendFuture<'_, Vec<InterfaceAddress>> {
            // Unfiltered on purpose: cleanup must filter by owner itself.
            Box::pin(async move { Ok(self.0.lock().unwrap().addresses.clone()) })
        }

        fn owned_profile_interfaces(
            &self,
            network: NetworkId,
        ) -> BackendFuture<'_, Vec<NetworkInterfaceHandle>> {
            Box::pin(async move {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .profiles
                    .iter()
                    .filter(|(owner, _)| *owner == network)
                    .map(|(_, interface)| interface.clone())
                    .collect())
            })
        }

        fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                self.0
                    .lock()
                    .unwrap()
                    .addresses
                    .retain(|existing| existing != &address);
                Ok(())
            })
        }

        fn remove_owned_interface(
            &self,
            network: NetworkId,
            interface: NetworkInterfaceHandle,
        ) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                let mut world = self.0.lock().unwrap();
                if world.unremovable.contains(&interface) {
                    return Err(CoreError::new(ErrorKind::Internal, "profile is busy"));
                }
                world
                    .profiles
                    .retain(|(owner, existing)| !(*owner == network && existing == &interface));
                Ok(())
            })
        }

        fn observe_panu(&self, _peer: PeerHandle) -> BackendFuture<'_, Option<PanAttachment>> {
            Box::pin(async move { Ok(self.0.lock().unwrap().panu.clone()) })
        }

        fn disconnect_panu(&self, expected: PanAttachment) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                let mut world = self.0.lock().unwrap();
                if world.panu.as_ref() == Some(&expected) {
                    world.panu = None;
                    world.disconnected += 1;
                }
                Ok(())
            })
        }
    }

    impl OwnedInventory for Fake {
        fn owned_resources(&self) -> BackendFuture<'_, Vec<(NetworkId, OwnedKind)>> {
            Box::pin(async move {
                let world = self.0.lock().unwrap();
                if world.fail_inventory {
                    return Err(CoreError::new(
                        ErrorKind::Internal,
                        "NetworkManager went away",
                    ));
                }
                let mut owned: Vec<_> = world
                    .addresses
                    .iter()
                    .map(|address| (address.owner, OwnedKind::Address))
                    .collect();
                owned.extend(
                    world
                        .profiles
                        .iter()
                        .map(|(owner, _)| (*owner, OwnedKind::Profile)),
                );
                Ok(owned)
            })
        }
    }

    /// A state directory with `networks` in its membership registry and, optionally, a client
    /// record for the first one.
    fn state(networks: &[NetworkId], client: bool) -> RemovalPaths {
        let root = std::env::temp_dir().join(format!(
            "blueroute-removal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = RemovalPaths {
            state_directory: root.clone(),
            memberships: root.join("memberships-v1"),
            client_record: root.join("client-record"),
        };
        let mut registry = MembershipRegistry::default();
        for network in networks {
            registry.remember_network(NetworkMembership::new(
                *network,
                DisplayName::new("Owned").unwrap(),
            ));
        }
        NetworkMembershipStore::new(paths.memberships.clone())
            .save(&registry)
            .unwrap();
        if client {
            ClientRecordStore::new(paths.client_record.clone())
                .save(Some(
                    &ClientRecord::new(
                        networks[0],
                        blueroute_core::NodeId::from_bytes([0x33; 16]),
                        Some(host_device()),
                    )
                    .unwrap(),
                ))
                .unwrap();
        }
        paths
    }

    fn run(
        paths: &RemovalPaths,
        fake: Fake,
        bluetooth: bool,
    ) -> (Result<RemovalReport, CoreError>, Fake) {
        let shared = std::sync::Arc::new(fake);
        let backend = shared.clone();
        let outcome = future::block_on(run_removal_cleanup(
            paths,
            || async move { Ok((ArcFake(backend), bluetooth)) },
            CLEANUP_BOUND,
        ));
        let fake = std::sync::Arc::try_unwrap(shared)
            .ok()
            .expect("backend released");
        (outcome, fake)
    }

    /// Lets a test keep the fake after cleanup consumed its backend.
    struct ArcFake(std::sync::Arc<Fake>);

    impl ClientLeaveBackend for ArcFake {
        fn owned_addresses(&self, network: NetworkId) -> BackendFuture<'_, Vec<InterfaceAddress>> {
            self.0.owned_addresses(network)
        }
        fn owned_profile_interfaces(
            &self,
            network: NetworkId,
        ) -> BackendFuture<'_, Vec<NetworkInterfaceHandle>> {
            self.0.owned_profile_interfaces(network)
        }
        fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
            self.0.remove_address(address)
        }
        fn remove_owned_interface(
            &self,
            network: NetworkId,
            interface: NetworkInterfaceHandle,
        ) -> BackendFuture<'_, ()> {
            self.0.remove_owned_interface(network, interface)
        }
        fn observe_panu(&self, peer: PeerHandle) -> BackendFuture<'_, Option<PanAttachment>> {
            self.0.observe_panu(peer)
        }
        fn disconnect_panu(&self, expected: PanAttachment) -> BackendFuture<'_, ()> {
            self.0.disconnect_panu(expected)
        }
    }

    impl OwnedInventory for ArcFake {
        fn owned_resources(&self) -> BackendFuture<'_, Vec<(NetworkId, OwnedKind)>> {
            self.0.owned_resources()
        }
    }

    #[test]
    fn owned_state_of_known_networks_goes_and_everything_else_stays() {
        let paths = state(&[net(1), net(2)], false);
        let fake = Fake::with(|world| {
            world.addresses = vec![
                address(net(1), "br-a"),
                address(net(2), "br-b"),
                address(net(9), "br-foreign"),
            ];
            world.profiles = vec![
                (net(1), iface("br-a")),
                (net(2), iface("br-b")),
                (net(9), iface("br-foreign")),
            ];
        });
        let (outcome, fake) = run(&paths, fake, true);
        let report = outcome.unwrap();
        assert!(report.succeeded(), "{report:?}");
        assert_eq!(
            (report.networks, report.remaining, report.unresolved),
            (2, 0, 2)
        );
        // Another installation's (or a purged one's) state is preserved untouched.
        fake.world(|world| {
            assert_eq!(world.addresses, vec![address(net(9), "br-foreign")]);
            assert_eq!(world.profiles, vec![(net(9), iface("br-foreign"))]);
        });
        assert!(removal_pending(&paths.state_directory));
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn a_proven_owned_panu_link_is_disconnected_first_and_only_with_bluetooth() {
        for bluetooth in [true, false] {
            let paths = state(&[net(1)], true);
            let fake = Fake::with(|world| {
                world.addresses = vec![address(net(1), "bnep0")];
                world.profiles = vec![(net(1), iface("bnep0"))];
                world.panu = Some(PanAttachment {
                    role: PanRole::Panu,
                    interface: iface("bnep0"),
                    peer: Some(host_device()),
                });
            });
            let (outcome, fake) = run(&paths, fake, bluetooth);
            let report = outcome.unwrap();
            assert!(report.succeeded(), "{report:?}");
            assert_eq!(report.bluetooth, bluetooth);
            // Without Bluetooth, NetworkManager cleanup still completes.
            assert_eq!(
                fake.world(|world| world.disconnected),
                usize::from(bluetooth)
            );
            assert!(fake.world(|world| world.profiles.is_empty()));
            std::fs::remove_dir_all(&paths.state_directory).unwrap();
        }
    }

    #[test]
    fn a_foreign_panu_link_is_left_alone() {
        let paths = state(&[net(1)], true);
        let foreign = PanAttachment {
            role: PanRole::Panu,
            interface: iface("bnep7"),
            peer: Some(host_device()),
        };
        let fake = Fake::with(|world| {
            world.profiles = vec![(net(1), iface("bnep0"))];
            world.panu = Some(foreign.clone());
        });
        let (outcome, fake) = run(&paths, fake, true);
        assert!(outcome.unwrap().succeeded());
        assert_eq!(fake.world(|world| world.panu.clone()), Some(foreign));
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn a_partial_failure_reports_what_remains_and_keeps_going() {
        let paths = state(&[net(1), net(2)], false);
        let fake = Fake::with(|world| {
            world.profiles = vec![(net(1), iface("br-a")), (net(2), iface("br-b"))];
            world.unremovable = vec![iface("br-a")];
        });
        let (outcome, fake) = run(&paths, fake, true);
        let report = outcome.unwrap();
        assert!(!report.succeeded());
        assert_eq!((report.failed_networks, report.remaining), (1, 1));
        // The other network was still cleaned, and membership is not cleared to hide anything.
        assert_eq!(
            fake.world(|world| world.profiles.clone()),
            vec![(net(1), iface("br-a"))]
        );
        assert_eq!(
            NetworkMembershipStore::new(paths.memberships.clone())
                .load()
                .unwrap()
                .networks()
                .count(),
            2
        );

        // A retry reobserves and finishes once the obstacle is gone.
        let retry = Fake::with(|world| world.profiles = vec![(net(1), iface("br-a"))]);
        let (outcome, _) = run(&paths, retry, true);
        assert!(outcome.unwrap().succeeded());
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn repeated_cleanup_is_a_successful_no_op() {
        let paths = state(&[net(1)], false);
        let fake = Fake::with(|world| world.profiles = vec![(net(1), iface("br-a"))]);
        let (first, fake) = run(&paths, fake, true);
        assert!(first.unwrap().succeeded());
        let (second, _) = run(&paths, fake, true);
        assert_eq!(second.unwrap().remaining, 0);
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn an_unreachable_or_failing_backend_is_a_failure_not_a_success() {
        let paths = state(&[net(1)], false);
        let unreachable = future::block_on(run_removal_cleanup(
            &paths,
            || async {
                Err::<(Fake, bool), _>(CoreError::new(ErrorKind::Internal, "no NetworkManager"))
            },
            CLEANUP_BOUND,
        ));
        assert!(unreachable.is_err());
        let blind = Fake::with(|world| world.fail_inventory = true);
        let (outcome, _) = run(&paths, blind, true);
        assert!(
            outcome.is_err(),
            "an unverifiable result must not count as success"
        );
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn with_no_known_network_nothing_is_contacted() {
        let paths = state(&[], false);
        let outcome = future::block_on(run_removal_cleanup(
            &paths,
            || async {
                Err::<(Fake, bool), _>(CoreError::new(ErrorKind::Internal, "must not connect"))
            },
            CLEANUP_BOUND,
        ));
        assert_eq!(outcome.unwrap(), RemovalReport::default());
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn cleanup_times_out_instead_of_hanging() {
        let paths = state(&[net(1)], false);
        let outcome = future::block_on(run_removal_cleanup(
            &paths,
            || async {
                future::pending::<()>().await;
                Ok((Fake::default(), true))
            },
            Duration::from_millis(50),
        ));
        assert_eq!(
            outcome.unwrap_err().kind(),
            ErrorKind::NetworkBackendUnavailable
        );
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn a_running_daemon_blocks_cleanup_and_a_pending_removal_blocks_the_daemon() {
        let paths = state(&[net(1)], false);
        // A daemon holds the instance lock: cleanup refuses and changes nothing.
        let daemon = acquire_instance_lock(&paths.state_directory).unwrap();
        let fake = Fake::with(|world| world.profiles = vec![(net(1), iface("br-a"))]);
        let (outcome, fake) = run(&paths, fake, true);
        assert_eq!(outcome.unwrap_err().kind(), ErrorKind::InvalidState);
        assert!(!removal_pending(&paths.state_directory));
        assert_eq!(fake.world(|world| world.profiles.len()), 1);
        drop(daemon);

        // After cleanup, the marker stays: a daemon must refuse to start.
        let (outcome, _) = run(&paths, fake, true);
        assert!(outcome.unwrap().succeeded());
        assert!(removal_pending(&paths.state_directory));
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn an_unreadable_registry_fails_before_anything_is_removed() {
        let paths = state(&[net(1)], false);
        std::fs::write(&paths.memberships, b"not a registry").unwrap();
        let fake = Fake::with(|world| world.profiles = vec![(net(1), iface("br-a"))]);
        let (outcome, fake) = run(&paths, fake, true);
        assert!(outcome.is_err());
        assert_eq!(fake.world(|world| world.profiles.len()), 1);
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    // FU-04 regression (docs/CODE_REVIEW_FOLLOWUP_TODO.md, REV-04, REMOVE-1..3). It fails on
    // 6d200d0, where cleanup without Bluetooth removes the proof, reports success, and leaves a
    // later Bluetooth-enabled retry unable to disconnect the owned link.
    #[test]
    #[ignore = "FU-04 regression: fails on 6d200d0 until FU-04 is fixed"]
    fn fu_04_unresolved_client_teardown_fails_and_keeps_its_proof_for_the_retry() {
        let owned_link = PanAttachment {
            role: PanRole::Panu,
            interface: iface("bnep0"),
            peer: Some(host_device()),
        };
        // Network 1: a joined client with a live owned PANU; network 2: a host-only network.
        let paths = state(&[net(1), net(2)], true);
        let fake = Fake::with(|world| {
            world.addresses = vec![address(net(1), "bnep0"), address(net(2), "br-b")];
            world.profiles = vec![(net(1), iface("bnep0")), (net(2), iface("br-b"))];
            world.panu = Some(owned_link.clone());
        });

        // Bluetooth is unavailable: the link cannot be observed, so it is unresolved.
        let (outcome, fake) = run(&paths, fake, false);
        let report = outcome.expect("cleanup ran");
        assert!(
            !report.succeeded(),
            "unobserved client teardown reported success: {report:?}"
        );
        fake.world(|world| {
            // The proof for the retry is kept …
            assert_eq!(world.profiles, vec![(net(1), iface("bnep0"))]);
            assert_eq!(world.panu, Some(owned_link.clone()));
            // … while independent safe work for the other network still happened.
            assert!(
                world
                    .addresses
                    .iter()
                    .all(|address| address.owner != net(2))
            );
        });

        // Bluetooth is back: the retry proves the link ours, disconnects it, and finishes.
        let (outcome, fake) = run(&paths, fake, true);
        assert!(outcome.unwrap().succeeded());
        fake.world(|world| {
            assert_eq!(world.disconnected, 1);
            assert!(world.profiles.is_empty() && world.addresses.is_empty());
        });
        let (outcome, _) = run(&paths, fake, true);
        assert!(
            outcome.unwrap().succeeded(),
            "a repeat after full cleanup must succeed"
        );
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }
}

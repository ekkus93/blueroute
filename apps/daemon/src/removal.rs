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
use blueroute_core::{CoreError, ErrorKind, MembershipRole, MembershipState, NetworkId};
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
    /// Joined-client networks whose PANU link could not be observed (Bluetooth unavailable, or
    /// no recorded host device). Their owned state is kept as proof for a retry, and cleanup
    /// fails (FU-04, REMOVE-1).
    pub unresolved_links: usize,
    /// Networks whose role cleanup cannot establish: an active membership with an unknown role,
    /// or a client record with no membership. Nothing of theirs is touched (their owned state is
    /// the proof for a later, evidence-based retry), and cleanup fails (LC-CLEANUP, SCOPE-2).
    pub unresolved_roles: usize,
    /// Whether Bluetooth was reachable, so a proven-owned PANU link could be disconnected.
    pub bluetooth: bool,
}

impl RemovalReport {
    pub fn succeeded(&self) -> bool {
        self.failed_networks == 0
            && self.remaining == 0
            && self.unresolved_links == 0
            && self.unresolved_roles == 0
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

/// How cleanup classifies one known network, from its persisted membership and the client
/// record (LC-CLEANUP, SCOPE-1). See `docs/CR-F5-REMOVAL-CLEANUP.md` for the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeClass {
    /// `Member`/`Host`: a hosted network. Only NetworkManager state can be ours. A client record
    /// naming it does not override the known role.
    Host,
    /// `NotMember`/none: a completed leave. Whatever owned state is left is NetworkManager state,
    /// and a client record left behind by that leave creates no client obligation.
    CompletedLeave,
    /// `Member`/`Client`: a joined client. A PANU link may exist and must be observed gone before
    /// its proof (the owned profile and address) is removed.
    Client,
    /// `Member` with no role: the store accepts it as "unknown". Cleanup cannot prove there is no
    /// client work, so it touches nothing and fails, with or without a client record or
    /// Bluetooth.
    UnknownRole,
    /// A client record for a network the registry does not hold: neither a completed leave nor a
    /// known role, so it is unresolved too.
    OrphanClientRecord,
}

/// What cleanup must do for one known network.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkScope {
    pub class: ScopeClass,
    /// Whether the client record names this network (provenance, for the log and tests).
    pub recorded: bool,
    /// The recorded host device, for a `Client` network only: an untrusted hint, used only to
    /// find a PANU link to prove. It never authorizes a disconnect on its own.
    pub host_hint: Option<PeerHandle>,
}

impl NetworkScope {
    /// Whether cleanup must leave this network untouched and fail.
    pub fn role_unresolved(&self) -> bool {
        matches!(
            self.class,
            ScopeClass::UnknownRole | ScopeClass::OrphanClientRecord
        )
    }
}

/// Classify every network this installation knows. Fails only when a store cannot be read or
/// validated: ownership could not be proven, so nothing is removed.
pub fn removal_scope(paths: &RemovalPaths) -> Result<BTreeMap<NetworkId, NetworkScope>, CoreError> {
    let registry = NetworkMembershipStore::new(paths.memberships.clone()).load()?;
    let mut known: BTreeMap<NetworkId, NetworkScope> = registry
        .networks()
        .map(|membership| {
            let class = match (membership.state, membership.role) {
                (MembershipState::Member, Some(MembershipRole::Host)) => ScopeClass::Host,
                (MembershipState::Member, Some(MembershipRole::Client)) => ScopeClass::Client,
                (MembershipState::NotMember, None) => ScopeClass::CompletedLeave,
                // `Member` without a role, and anything the store should never yield.
                _ => ScopeClass::UnknownRole,
            };
            (
                membership.network_id,
                NetworkScope {
                    class,
                    recorded: false,
                    host_hint: None,
                },
            )
        })
        .collect();
    // The persisted role decides the class. The client record only supplies the host hint of a
    // client network, and a record for a network with no membership is unresolved.
    if let Some(record) = ClientRecordStore::new(paths.client_record.clone()).load()? {
        let scope = known.entry(record.network()).or_insert(NetworkScope {
            class: ScopeClass::OrphanClientRecord,
            recorded: false,
            host_hint: None,
        });
        scope.recorded = true;
        if scope.class == ScopeClass::Client {
            scope.host_hint = record.device_hint().cloned();
        }
    }
    Ok(known)
}

/// Remove every known network's owned state, then reobserve. One network's failure does not
/// stop the others; nothing without a known network's ownership is touched.
///
/// A host network needs only NetworkManager. A joined-client network may have a PANU link, and
/// "Bluetooth unreachable" is not evidence that it is gone: unless Bluetooth is up and the host
/// device is recorded, its owned profile and address are left in place (they prove the link is
/// ours on a retry) and it counts as unresolved, which fails cleanup (FU-04).
pub async fn remove_known_networks(
    backend: &dyn OwnedInventory,
    known: &BTreeMap<NetworkId, NetworkScope>,
    bluetooth: bool,
) -> Result<RemovalReport, CoreError> {
    let mut report = RemovalReport {
        networks: known.len(),
        bluetooth,
        ..RemovalReport::default()
    };
    for (network, scope) in known {
        if scope.role_unresolved() {
            report.unresolved_roles += 1;
            continue;
        }
        let client = scope.class == ScopeClass::Client;
        let hint = match (client, bluetooth, scope.host_hint.as_ref()) {
            (false, _, _) => None,
            (true, true, Some(hint)) => {
                // "No link observed" only counts if BlueZ actually has the host device: a
                // missing object (no adapter, a stale or renumbered path) is inconclusive.
                match backend.panu_device_present(hint.clone()).await {
                    Ok(true) => Some(hint),
                    Ok(false) => {
                        report.unresolved_links += 1;
                        continue;
                    }
                    Err(_) => {
                        report.failed_networks += 1;
                        continue;
                    }
                }
            }
            (true, _, _) => {
                report.unresolved_links += 1;
                continue;
            }
        };
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

/// The maintenance helper's exit status and its single log line: 0 only for a full success,
/// 1 for any failure (including unresolved client links). Counts and a coarse class only.
pub fn cleanup_outcome(outcome: &Result<RemovalReport, CoreError>) -> (i32, serde_json::Value) {
    match outcome {
        Ok(report) => (
            i32::from(!report.succeeded()),
            serde_json::json!({
                "component": "removal_cleanup",
                "outcome": if report.succeeded() { "succeeded" } else { "failed" },
                "networks": report.networks,
                "failed_networks": report.failed_networks,
                "remaining_owned_resources": report.remaining,
                "unresolved_foreign_resources": report.unresolved,
                "unresolved_client_links": report.unresolved_links,
                "unresolved_roles": report.unresolved_roles,
                "bluetooth_available": report.bluetooth,
            }),
        ),
        Err(error) => (
            1,
            serde_json::json!({
                "component": "removal_cleanup",
                "outcome": "failed",
                "failure_class": format!("{:?}", error.kind()),
            }),
        ),
    }
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
        /// The link survives a disconnect request.
        sticky_link: bool,
        /// BlueZ has no object for the recorded host device.
        device_absent: bool,
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

        fn panu_device_present(&self, _peer: PeerHandle) -> BackendFuture<'_, bool> {
            Box::pin(async move { Ok(!self.0.lock().unwrap().device_absent) })
        }

        fn observe_panu(&self, _peer: PeerHandle) -> BackendFuture<'_, Option<PanAttachment>> {
            Box::pin(async move { Ok(self.0.lock().unwrap().panu.clone()) })
        }

        fn disconnect_panu(&self, expected: PanAttachment) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                let mut world = self.0.lock().unwrap();
                if world.panu.as_ref() == Some(&expected) && !world.sticky_link {
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
        for (index, network) in networks.iter().enumerate() {
            let mut membership =
                NetworkMembership::new(*network, DisplayName::new("Owned").unwrap());
            if client && index == 0 {
                membership.state = blueroute_core::MembershipState::Member;
                membership.role = Some(blueroute_core::MembershipRole::Client);
            }
            registry.remember_network(membership);
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
        fn panu_device_present(&self, peer: PeerHandle) -> BackendFuture<'_, bool> {
            self.0.panu_device_present(peer)
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
    fn a_proven_owned_panu_link_is_disconnected_with_bluetooth_and_unresolved_without() {
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
            assert_eq!(report.bluetooth, bluetooth);
            if bluetooth {
                assert!(report.succeeded(), "{report:?}");
                assert_eq!(fake.world(|world| world.disconnected), 1);
                assert!(fake.world(|world| world.profiles.is_empty()));
            } else {
                // Corrected (FU-04): an unobservable client link is unresolved, not success, and
                // the proof it needs stays.
                assert!(!report.succeeded(), "{report:?}");
                assert_eq!(report.unresolved_links, 1);
                assert_eq!(fake.world(|world| world.disconnected), 0);
                assert_eq!(fake.world(|world| world.profiles.len()), 1);
            }
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

    fn owned_link(interface: &str) -> PanAttachment {
        PanAttachment {
            role: PanRole::Panu,
            interface: iface(interface),
            peer: Some(host_device()),
        }
    }

    fn client_world(panu: Option<PanAttachment>) -> Fake {
        Fake::with(|world| {
            world.addresses = vec![address(net(1), "bnep0")];
            world.profiles = vec![(net(1), iface("bnep0"))];
            world.panu = panu;
        })
    }

    #[test]
    fn fu_04_an_already_absent_link_with_bluetooth_up_is_proven_gone() {
        // Also the state after an interruption between disconnecting and removing the proof.
        let paths = state(&[net(1)], true);
        let (outcome, fake) = run(&paths, client_world(None), true);
        assert!(outcome.unwrap().succeeded());
        assert!(fake.world(|world| world.profiles.is_empty() && world.addresses.is_empty()));
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn fu_04_a_disconnect_that_does_not_take_keeps_the_proof() {
        let paths = state(&[net(1)], true);
        let fake = client_world(Some(owned_link("bnep0")));
        fake.0.lock().unwrap().sticky_link = true;
        let (outcome, fake) = run(&paths, fake, true);
        let report = outcome.unwrap();
        assert!(!report.succeeded(), "{report:?}");
        assert_eq!(report.failed_networks, 1);
        // Nothing was removed: the retry can still prove the link ours.
        assert_eq!(fake.world(|world| world.profiles.len()), 1);
        assert_eq!(fake.world(|world| world.addresses.len()), 1);
        fake.0.lock().unwrap().sticky_link = false;
        let (outcome, _) = run(&paths, fake, true);
        assert!(outcome.unwrap().succeeded());
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn fu_04_a_link_to_another_device_on_a_reused_interface_name_is_left_alone() {
        let paths = state(&[net(1)], true);
        let other = PanAttachment {
            role: PanRole::Panu,
            interface: iface("bnep0"),
            peer: Some(PeerHandle::new("/org/bluez/hci0/dev_BB_00_00_00_00_02").unwrap()),
        };
        let (outcome, fake) = run(&paths, client_world(Some(other.clone())), true);
        assert!(outcome.unwrap().succeeded());
        // Only the recorded host's link could be ours; this one was not touched.
        assert_eq!(fake.world(|world| world.panu.clone()), Some(other));
        assert_eq!(fake.world(|world| world.disconnected), 0);
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn fu_04_without_proof_a_live_link_is_never_disconnected() {
        // The owned profile and address are already gone (vanished proof), or name a different
        // interface (conflicting proof): the live link cannot be proven ours.
        for profiles in [Vec::new(), vec![(net(1), iface("bnep3"))]] {
            let paths = state(&[net(1)], true);
            let fake = Fake::with(|world| {
                world.profiles = profiles.clone();
                world.panu = Some(owned_link("bnep0"));
            });
            let (outcome, fake) = run(&paths, fake, true);
            assert!(outcome.unwrap().succeeded());
            assert_eq!(fake.world(|world| world.disconnected), 0);
            assert!(fake.world(|world| world.panu.is_some()));
            std::fs::remove_dir_all(&paths.state_directory).unwrap();
        }
    }

    #[test]
    fn fu_04_a_client_network_known_only_by_role_is_unresolved_without_a_host_device() {
        // A joined client whose client record could not be written: the role still says client.
        let paths = state(&[], false);
        let mut registry = MembershipRegistry::default();
        let mut membership = NetworkMembership::new(net(1), DisplayName::new("Joined").unwrap());
        membership.state = blueroute_core::MembershipState::Member;
        membership.role = Some(blueroute_core::MembershipRole::Client);
        registry.remember_network(membership);
        NetworkMembershipStore::new(paths.memberships.clone())
            .save(&registry)
            .unwrap();
        let (outcome, fake) = run(&paths, client_world(Some(owned_link("bnep0"))), true);
        let report = outcome.unwrap();
        assert!(!report.succeeded());
        assert_eq!(report.unresolved_links, 1);
        assert_eq!(fake.world(|world| world.profiles.len()), 1);
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn an_unresolved_client_link_makes_the_helper_exit_nonzero() {
        let unresolved = RemovalReport {
            networks: 1,
            unresolved_links: 1,
            ..RemovalReport::default()
        };
        let (code, line) = cleanup_outcome(&Ok(unresolved));
        assert_eq!(code, 1);
        assert_eq!(line["outcome"], "failed");
        assert_eq!(line["unresolved_client_links"], 1);
        let (code, line) = cleanup_outcome(&Ok(RemovalReport::default()));
        assert_eq!((code, line["outcome"].as_str()), (0, Some("succeeded")));
    }

    #[test]
    fn fu_04_a_missing_host_device_is_inconclusive_even_with_bluetooth_running() {
        // bluetoothd answers but has no object for the recorded host (no adapter, or a stale
        // path): "no link observed" proves nothing, so the proof stays and cleanup fails.
        let paths = state(&[net(1), net(2)], true);
        let fake = Fake::with(|world| {
            world.profiles = vec![(net(1), iface("bnep0")), (net(2), iface("br-b"))];
            world.device_absent = true;
        });
        let (outcome, fake) = run(&paths, fake, true);
        let report = outcome.unwrap();
        assert!(!report.succeeded(), "{report:?}");
        assert_eq!(report.unresolved_links, 1);
        assert_eq!(
            fake.world(|world| world.profiles.clone()),
            vec![(net(1), iface("bnep0"))],
            "the client proof must stay; the host network is still cleaned"
        );
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn fu_04_a_stale_client_record_does_not_block_removal() {
        // A leave succeeded but clearing the record failed: the registry no longer says client.
        let paths = state(&[net(1)], false);
        ClientRecordStore::new(paths.client_record.clone())
            .save(Some(
                &ClientRecord::new(
                    net(1),
                    blueroute_core::NodeId::from_bytes([0x33; 16]),
                    Some(host_device()),
                )
                .unwrap(),
            ))
            .unwrap();
        let fake = Fake::with(|world| world.profiles = vec![(net(1), iface("br-a"))]);
        let (outcome, fake) = run(&paths, fake, false);
        let report = outcome.unwrap();
        assert!(report.succeeded(), "{report:?}");
        assert!(fake.world(|world| world.profiles.is_empty()));
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    /// Saves `memberships` (network, state, role) and optionally a client record for `record`
    /// through the production stores.
    fn seeded(
        memberships: &[(
            NetworkId,
            blueroute_core::MembershipState,
            Option<MembershipRole>,
        )],
        record: Option<NetworkId>,
    ) -> RemovalPaths {
        let paths = state(&[], false);
        let mut registry = MembershipRegistry::default();
        for (network, state, role) in memberships {
            let mut membership =
                NetworkMembership::new(*network, DisplayName::new("Seen").unwrap());
            membership.state = *state;
            membership.role = *role;
            registry.remember_network(membership);
        }
        NetworkMembershipStore::new(paths.memberships.clone())
            .save(&registry)
            .unwrap();
        if let Some(network) = record {
            ClientRecordStore::new(paths.client_record.clone())
                .save(Some(
                    &ClientRecord::new(
                        network,
                        blueroute_core::NodeId::from_bytes([0x33; 16]),
                        Some(host_device()),
                    )
                    .unwrap(),
                ))
                .unwrap();
        }
        paths
    }

    // RFU-02 / LC-BASE-04 regression (docs/CODE_REVIEW_LIFECYCLE_TODO.md, SCOPE-1/2). On 75c1a01
    // a Member/None network with a matching client record is cleaned as host-only: without
    // Bluetooth its owned proof is deleted and cleanup reports success.
    #[test]
    fn lc_cleanup_an_unknown_active_role_with_a_client_record_stays_unresolved() {
        use blueroute_core::MembershipState::Member;
        let paths = seeded(&[(net(1), Member, None)], Some(net(1)));
        let fake = client_world(Some(owned_link("bnep0")));
        let (outcome, fake) = run(&paths, fake, false);
        let report = outcome.expect("cleanup ran");
        assert!(
            !report.succeeded(),
            "an unknown active role was cleaned as host-only: {report:?}"
        );
        fake.world(|world| {
            assert_eq!(world.profiles, vec![(net(1), iface("bnep0"))]);
            assert_eq!(world.addresses, vec![address(net(1), "bnep0")]);
            assert_eq!(world.disconnected, 0);
        });
        // Membership and client record are kept for a later, evidence-based retry.
        let registry = NetworkMembershipStore::new(paths.memberships.clone())
            .load()
            .unwrap();
        let kept = registry.network(&net(1)).expect("membership kept");
        assert_eq!((kept.state, kept.role), (Member, None));
        assert!(
            ClientRecordStore::new(paths.client_record.clone())
                .load()
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    fn record_for(paths: &RemovalPaths, network: NetworkId, hint: Option<PeerHandle>) {
        ClientRecordStore::new(paths.client_record.clone())
            .save(Some(
                &ClientRecord::new(
                    network,
                    blueroute_core::NodeId::from_bytes([0x33; 16]),
                    hint,
                )
                .unwrap(),
            ))
            .unwrap();
    }

    fn stored(paths: &RemovalPaths) -> (MembershipRegistry, Option<ClientRecord>) {
        (
            NetworkMembershipStore::new(paths.memberships.clone())
                .load()
                .unwrap(),
            ClientRecordStore::new(paths.client_record.clone())
                .load()
                .unwrap(),
        )
    }

    // LC-CLEANUP-05: Member/None stays unresolved through the production stores and scope, with
    // Bluetooth absent or present and with a record carrying a hint, one without, or none. A
    // confirmed host network beside it is still cleaned and a foreign network is preserved.
    #[test]
    fn lc_cleanup_an_unknown_role_is_unresolved_in_every_combination() {
        use blueroute_core::MembershipState::Member;
        let foreign = net(9);
        for bluetooth in [false, true] {
            for record in [Some(Some(host_device())), Some(None), None] {
                let label = format!("bluetooth {bluetooth}, record {record:?}");
                let paths = seeded(
                    &[
                        (net(1), Member, None),
                        (net(2), Member, Some(MembershipRole::Host)),
                    ],
                    None,
                );
                if let Some(hint) = record.clone() {
                    record_for(&paths, net(1), hint);
                }
                let before = stored(&paths);
                let fake = Fake::with(|world| {
                    world.addresses = vec![
                        address(net(1), "bnep0"),
                        address(net(2), "br-b"),
                        address(foreign, "br-f"),
                    ];
                    world.profiles = vec![
                        (net(1), iface("bnep0")),
                        (net(2), iface("br-b")),
                        (foreign, iface("br-f")),
                    ];
                    world.panu = Some(owned_link("bnep0"));
                });
                let (outcome, fake) = run(&paths, fake, bluetooth);
                let report = outcome.expect("cleanup ran");
                assert!(!report.succeeded(), "{label}: {report:?}");
                assert_eq!(report.unresolved_roles, 1, "{label}");
                assert_eq!(
                    report.unresolved, 2,
                    "{label}: the foreign state is reported"
                );
                let (code, line) = cleanup_outcome(&Ok(report));
                assert_eq!(
                    (code, &line["unresolved_roles"]),
                    (1, &serde_json::json!(1))
                );
                fake.world(|world| {
                    assert_eq!(
                        world.profiles,
                        vec![(net(1), iface("bnep0")), (foreign, iface("br-f"))],
                        "{label}: only the confirmed host network is cleaned"
                    );
                    assert_eq!(
                        world.addresses,
                        vec![address(net(1), "bnep0"), address(foreign, "br-f")],
                        "{label}"
                    );
                    assert_eq!(world.disconnected, 0, "{label}: no link is disconnected");
                    assert_eq!(world.panu, Some(owned_link("bnep0")), "{label}");
                });
                let after = stored(&paths);
                assert_eq!(
                    after.0.network(&net(1)).map(|m| (m.state, m.role)),
                    Some((Member, None)),
                    "{label}: the unknown role is not rewritten"
                );
                assert_eq!(after.1, before.1, "{label}: the client record is kept");
                std::fs::remove_dir_all(&paths.state_directory).unwrap();
            }
        }
    }

    #[test]
    fn lc_cleanup_an_unknown_role_fails_even_with_no_owned_state_left() {
        use blueroute_core::MembershipState::Member;
        let paths = seeded(&[(net(1), Member, None)], Some(net(1)));
        let (outcome, _) = run(&paths, Fake::default(), true);
        let report = outcome.unwrap();
        assert!(!report.succeeded(), "{report:?}");
        assert_eq!((report.remaining, report.unresolved_roles), (0, 1));
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn lc_cleanup_an_unknown_role_with_conflicting_proof_is_left_alone() {
        use blueroute_core::MembershipState::Member;
        // The owned profile names another interface than the live link: nothing proves it, and
        // nothing is touched either way.
        let paths = seeded(&[(net(1), Member, None)], Some(net(1)));
        let fake = Fake::with(|world| {
            world.profiles = vec![(net(1), iface("bnep3"))];
            world.panu = Some(owned_link("bnep0"));
        });
        let (outcome, fake) = run(&paths, fake, true);
        assert_eq!(outcome.unwrap().unresolved_roles, 1);
        assert_eq!(fake.world(|world| world.profiles.len()), 1);
        assert_eq!(fake.world(|world| world.disconnected), 0);
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    /// A persisted membership state and role.
    type Stored = (blueroute_core::MembershipState, Option<MembershipRole>);

    // LC-CLEANUP-01/06: every row of the classification table, through the production stores.
    #[test]
    fn lc_cleanup_scope_classifies_every_stored_combination() {
        use blueroute_core::MembershipState::{Member, NotMember};
        // (label, stored membership, client record present, expected class, takes the hint)
        let rows: [(&str, Option<Stored>, bool, ScopeClass, bool); 9] = [
            (
                "host",
                Some((Member, Some(MembershipRole::Host))),
                false,
                ScopeClass::Host,
                false,
            ),
            (
                "host, stale record",
                Some((Member, Some(MembershipRole::Host))),
                true,
                ScopeClass::Host,
                false,
            ),
            (
                "client",
                Some((Member, Some(MembershipRole::Client))),
                true,
                ScopeClass::Client,
                true,
            ),
            (
                "client, no record",
                Some((Member, Some(MembershipRole::Client))),
                false,
                ScopeClass::Client,
                false,
            ),
            (
                "unknown",
                Some((Member, None)),
                false,
                ScopeClass::UnknownRole,
                false,
            ),
            (
                "unknown, record",
                Some((Member, None)),
                true,
                ScopeClass::UnknownRole,
                false,
            ),
            (
                "left",
                Some((NotMember, None)),
                false,
                ScopeClass::CompletedLeave,
                false,
            ),
            (
                "left, stale record",
                Some((NotMember, None)),
                true,
                ScopeClass::CompletedLeave,
                false,
            ),
            (
                "record only",
                None,
                true,
                ScopeClass::OrphanClientRecord,
                false,
            ),
        ];
        for (label, membership, record, class, hinted) in rows {
            let memberships: Vec<_> = membership
                .into_iter()
                .map(|(state, role)| (net(1), state, role))
                .collect();
            let paths = seeded(&memberships, record.then_some(net(1)));
            let scope = removal_scope(&paths).unwrap();
            let entry = scope.get(&net(1)).expect(label);
            assert_eq!(entry.class, class, "{label}");
            assert_eq!(entry.recorded, record, "{label}");
            // Only a client network takes the record's host hint.
            assert_eq!(entry.host_hint.is_some(), hinted, "{label}");
            assert_eq!(
                entry.role_unresolved(),
                matches!(
                    class,
                    ScopeClass::UnknownRole | ScopeClass::OrphanClientRecord
                ),
                "{label}"
            );
            std::fs::remove_dir_all(&paths.state_directory).unwrap();
        }
    }

    #[test]
    fn lc_cleanup_a_client_record_without_a_membership_is_unresolved() {
        let paths = seeded(&[], Some(net(1)));
        let fake = client_world(Some(owned_link("bnep0")));
        let (outcome, fake) = run(&paths, fake, true);
        let report = outcome.unwrap();
        assert!(!report.succeeded(), "{report:?}");
        assert_eq!(report.unresolved_roles, 1);
        fake.world(|world| {
            assert_eq!(world.profiles.len(), 1);
            assert_eq!(
                world.disconnected, 0,
                "a hint alone never authorizes a disconnect"
            );
        });
        assert!(stored(&paths).1.is_some(), "the record is not cleared");
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    // LC-CLEANUP-07: evidence-based recovery. A retry rereads the stores; Bluetooth coming back
    // proves nothing on its own. Once the verified role is in the production store (seeded here
    // as the recovered fact), normal client cleanup finishes and repeats idempotently.
    #[test]
    fn lc_cleanup_recovery_needs_the_role_established_and_then_completes() {
        use blueroute_core::MembershipState::Member;
        let paths = seeded(&[(net(1), Member, None)], Some(net(1)));
        let fake = client_world(Some(owned_link("bnep0")));
        let (outcome, fake) = run(&paths, fake, false);
        assert_eq!(outcome.unwrap().unresolved_roles, 1);
        // Bluetooth back, role still unknown: still unresolved, still untouched.
        let (outcome, fake) = run(&paths, fake, true);
        assert_eq!(outcome.unwrap().unresolved_roles, 1);
        assert_eq!(fake.world(|world| world.disconnected), 0);
        // The role is established independently and stored; the retry rereads it.
        let paths_recovered = seeded(
            &[(net(1), Member, Some(MembershipRole::Client))],
            Some(net(1)),
        );
        std::fs::rename(&paths_recovered.memberships, &paths.memberships).unwrap();
        std::fs::remove_dir_all(&paths_recovered.state_directory).unwrap();
        let (outcome, fake) = run(&paths, fake, true);
        let report = outcome.unwrap();
        assert!(report.succeeded(), "{report:?}");
        fake.world(|world| {
            assert_eq!(world.disconnected, 1);
            assert!(world.profiles.is_empty() && world.addresses.is_empty());
        });
        let (outcome, _) = run(&paths, fake, true);
        assert!(outcome.unwrap().succeeded(), "a repeat must succeed");
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }

    #[test]
    fn lc_cleanup_a_recovered_host_role_finishes_with_host_cleanup() {
        use blueroute_core::MembershipState::Member;
        let paths = seeded(&[(net(1), Member, None)], None);
        let fake = Fake::with(|world| {
            world.addresses = vec![address(net(1), "br-a")];
            world.profiles = vec![(net(1), iface("br-a"))];
        });
        let (outcome, fake) = run(&paths, fake, false);
        assert_eq!(outcome.unwrap().unresolved_roles, 1);
        assert_eq!(fake.world(|world| world.profiles.len()), 1);
        let recovered = seeded(&[(net(1), Member, Some(MembershipRole::Host))], None);
        std::fs::rename(&recovered.memberships, &paths.memberships).unwrap();
        std::fs::remove_dir_all(&recovered.state_directory).unwrap();
        let (outcome, fake) = run(&paths, fake, false);
        assert!(outcome.unwrap().succeeded());
        assert!(fake.world(|world| world.profiles.is_empty() && world.addresses.is_empty()));
        std::fs::remove_dir_all(&paths.state_directory).unwrap();
    }
}

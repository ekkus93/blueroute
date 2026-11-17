//! TC-NM: the production `NetworkManagerBackend`, unchanged, against a fake NetworkManager on a
//! private bus (`networkmanager_backend/fake.rs`).
//!
//! `NetworkManagerBackend::connect_system()` uses the *system* bus. The outer test re-runs this
//! binary with `DBUS_SYSTEM_BUS_ADDRESS` pointed at the private session bus from
//! `dbus-run-session` (setting it in-process would need `unsafe`). The child runs every
//! `scenario_*` test one at a time, because each one owns `org.freedesktop.NetworkManager`:
//!
//! ```text
//! dbus-run-session -- cargo test -p blueroute-linux --test networkmanager_backend \
//!     --locked -- --ignored
//! ```
//!
//! The scenarios prove ownership at the D-Bus boundary (TEST_COVERAGE_SPEC COV-4): owned
//! profiles are created, changed and removed idempotently; foreign, other-network, malformed and
//! duplicate profiles make operations fail closed with no mutating call on any of them; and
//! NetworkManager failures map to `NetworkBackendUnavailable`.

#[path = "networkmanager_backend/fake.rs"]
mod fake;

use std::fs::File;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::time::{Duration, Instant};

use blueroute_core::{CoreError, ErrorKind, IpPrefix, NetworkId};
use blueroute_linux::{
    InterfaceAddress, IpNetworkBackend, LinuxRoute, NetworkInterfaceHandle, NetworkManagerBackend,
    NetworkStateBackend, NetworkStateEvent,
};
use fake::{Call, FakeNetworkManager, Settings, VERSION, clone_settings};
use zbus::zvariant::{OwnedValue, Str};

const CHILD: &str = "BLUEROUTE_NM_BACKEND_CHILD";
const SCENARIOS: usize = 10;
/// Bound for the whole child run, so a hang fails the test instead of the CI job.
const CHILD_BOUND: Duration = Duration::from_secs(180);
/// Bound for one subscription event; the backend polls every 250 ms.
const EVENT_BOUND: Duration = Duration::from_secs(10);
const DEVICE_TYPE_ETHERNET: u32 = 1;
const DEVICE_TYPE_BRIDGE: u32 = 13;
const ADD_FLAGS: u32 = 0x1 | 0x20;
const UPDATE_FLAGS: u32 = 0x1;

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn networkmanager_backend_scenarios_over_a_private_bus() {
    if std::env::var_os(CHILD).is_some() {
        return; // the child runs only the scenarios
    }
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("run under dbus-run-session");
    let log_path = format!(
        "{}/networkmanager_backend_child.log",
        env!("CARGO_TARGET_TMPDIR")
    );
    let log = File::create(&log_path).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--test-threads=1", "scenario_"])
        .env(CHILD, "1")
        .env("DBUS_SYSTEM_BUS_ADDRESS", address)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > CHILD_BOUND {
            child.kill().unwrap();
            child.wait().unwrap();
            let report = std::fs::read_to_string(&log_path).unwrap_or_default();
            panic!("scenarios did not finish within {CHILD_BOUND:?}:\n{report}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let report = std::fs::read_to_string(&log_path).unwrap();
    assert!(status.success(), "{report}");
    assert!(
        report.contains(&format!("test result: ok. {SCENARIOS} passed")),
        "expected {SCENARIOS} scenarios to run:\n{report}"
    );
}

/// `future`'s output, or a panic naming `what` if it takes longer than `EVENT_BOUND`.
async fn within<T>(what: &str, future: impl Future<Output = T>) -> T {
    let timeout = async {
        async_io::Timer::after(EVENT_BOUND).await;
        None
    };
    match futures_lite::future::or(async { Some(future.await) }, timeout).await {
        Some(value) => value,
        None => panic!("{what}: no result within {EVENT_BOUND:?}"),
    }
}

/// Run `body` only in the child, where the "system" bus is the private one.
fn scenario<F: Future<Output = ()>>(body: impl FnOnce() -> F) {
    if std::env::var_os(CHILD).is_some() {
        async_io::block_on(body());
    }
}

fn net_a() -> NetworkId {
    NetworkId::from_bytes([0xa1; 16])
}

fn net_b() -> NetworkId {
    NetworkId::from_bytes([0xb2; 16])
}

fn iface(name: &str) -> NetworkInterfaceHandle {
    NetworkInterfaceHandle::new(name).unwrap()
}

fn text(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value.to_owned()))
}

fn address(owner: NetworkId, interface: &str) -> InterfaceAddress {
    InterfaceAddress {
        interface: iface(interface),
        prefix: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 1)), 24).unwrap(),
        owner,
    }
}

fn route(owner: NetworkId, interface: &str) -> LinuxRoute {
    LinuxRoute {
        destination: IpPrefix::new(IpAddr::V4(Ipv4Addr::new(10, 43, 0, 0)), 24).unwrap(),
        via: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 2))),
        interface: iface(interface),
        metric: 77,
        owner,
    }
}

/// BlueRoute ownership metadata as NetworkManager stores it: `user.data`, an `a{ss}`.
fn metadata(owner: &str, kind: Option<&str>, schema: Option<&str>) -> OwnedValue {
    let mut data = std::collections::HashMap::<String, String>::new();
    data.insert("org.blueroute.owner".to_owned(), owner.to_owned());
    if let Some(kind) = kind {
        data.insert("org.blueroute.kind".to_owned(), kind.to_owned());
    }
    if let Some(schema) = schema {
        data.insert("org.blueroute.schema".to_owned(), schema.to_owned());
    }
    OwnedValue::from(data)
}

fn profile(id: &str, connection_type: &str, interface: &str, user: Option<OwnedValue>) -> Settings {
    let mut connection = std::collections::HashMap::new();
    connection.insert("id".to_owned(), text(id));
    connection.insert("uuid".to_owned(), text(&format!("uuid-{id}")));
    connection.insert("type".to_owned(), text(connection_type));
    connection.insert("interface-name".to_owned(), text(interface));
    let mut ipv4 = std::collections::HashMap::new();
    ipv4.insert("method".to_owned(), text("disabled"));
    let mut ipv6 = std::collections::HashMap::new();
    ipv6.insert("method".to_owned(), text("disabled"));
    let mut settings = Settings::new();
    settings.insert("connection".to_owned(), connection);
    settings.insert("ipv4".to_owned(), ipv4);
    settings.insert("ipv6".to_owned(), ipv6);
    if let Some(user) = user {
        let mut group = std::collections::HashMap::new();
        group.insert("data".to_owned(), user);
        settings.insert("user".to_owned(), group);
    }
    settings
}

fn owned(owner: NetworkId, interface: &str, kind: &str) -> Settings {
    let connection_type = if kind == "bridge" {
        "bridge"
    } else {
        "generic"
    };
    let id = format!("blueroute-{kind}-{interface}-{owner}");
    let user = metadata(&owner.to_string(), Some(kind), Some("1"));
    profile(&id, connection_type, interface, Some(user))
}

fn foreign(id: &str, interface: &str) -> Settings {
    profile(id, "802-3-ethernet", interface, None)
}

/// `(method, profile)` for each call, asserting none of them was a scripted failure.
fn summary(calls: &[Call]) -> Vec<(&'static str, String)> {
    assert!(calls.iter().all(|call| !call.failed), "{calls:#?}");
    calls
        .iter()
        .map(|call| (call.method, call.profile.clone()))
        .collect()
}

fn calls_then_clear(fake: &FakeNetworkManager) -> Vec<(&'static str, String)> {
    let calls = summary(&fake.calls());
    fake.clear_calls();
    calls
}

/// Snapshots of profiles that must never change.
fn snapshot(fake: &FakeNetworkManager, paths: &[&str]) -> Vec<(String, Settings)> {
    paths
        .iter()
        .map(|path| {
            (
                path.to_string(),
                fake.settings(path).expect("seeded profile"),
            )
        })
        .collect()
}

fn assert_unchanged(fake: &FakeNetworkManager, snapshots: &[(String, Settings)]) {
    for (path, before) in snapshots {
        let after = fake
            .settings(path)
            .unwrap_or_else(|| panic!("{path} was deleted"));
        assert!(after == clone_settings(before), "{path} was modified");
    }
}

fn kind_of<T: std::fmt::Debug>(result: Result<T, CoreError>, case: &str) -> ErrorKind {
    match result {
        Ok(value) => panic!("{case}: unexpectedly succeeded with {value:?}"),
        Err(error) => error.kind(),
    }
}

/// Every mutating backend operation for `owner` on `interface`, in a fixed order.
async fn every_mutation(
    backend: &NetworkManagerBackend,
    owner: NetworkId,
    interface: &str,
) -> Vec<(&'static str, Result<(), CoreError>)> {
    let mut results = vec![
        (
            "ensure_bridge",
            backend
                .ensure_bridge(owner, iface(interface))
                .await
                .map(drop),
        ),
        (
            "ensure_address",
            backend.ensure_address(address(owner, interface)).await,
        ),
        (
            "ensure_route",
            backend.ensure_route(route(owner, interface)).await,
        ),
    ];
    results.extend(every_removal(backend, owner, interface).await);
    results
}

/// Every removal operation for `owner` on `interface`, in a fixed order.
async fn every_removal(
    backend: &NetworkManagerBackend,
    owner: NetworkId,
    interface: &str,
) -> Vec<(&'static str, Result<(), CoreError>)> {
    vec![
        (
            "remove_owned_interface",
            backend
                .remove_owned_interface(owner, iface(interface))
                .await,
        ),
        (
            "remove_address",
            backend.remove_address(address(owner, interface)).await,
        ),
        (
            "remove_route",
            backend.remove_route(route(owner, interface)).await,
        ),
    ]
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_owned_bridge_lifecycle_is_idempotent_and_leaves_others_untouched() {
    scenario(|| async {
        let fake = FakeNetworkManager::start().await.unwrap();
        let wired = fake.add_profile(foreign("Wired", "eth0"));
        let eth0 = fake.add_device("eth0", DEVICE_TYPE_ETHERNET, Some(&wired));
        let other = fake.add_profile(owned(net_b(), "br-other", "bridge"));
        let untouched = snapshot(&fake, &[&wired, &other]);
        let backend = NetworkManagerBackend::connect_system().await.unwrap();
        let a = net_a();

        assert_eq!(backend.version().await.unwrap(), VERSION);
        assert_eq!(
            backend.service_owner().await.unwrap(),
            Some(fake.unique_name())
        );

        // Create and activate the bridge profile.
        let bridge = backend.ensure_bridge(a, iface("br-blue0")).await.unwrap();
        let path = bridge.handle.as_str().to_owned();
        assert_eq!(bridge.owner, Some(a));
        assert_eq!(bridge.connection_type, "bridge");
        assert_eq!(bridge.interface, Some(iface("br-blue0")));
        let calls = fake.calls();
        assert_eq!(calls[0].flags, Some(ADD_FLAGS), "AddConnection2 flags");
        assert_eq!(calls[1].device.as_deref(), Some("/"), "virtual activation");
        assert_eq!(
            calls_then_clear(&fake),
            [
                ("AddConnection2", path.clone()),
                ("ActivateConnection", path.clone())
            ]
        );
        let stored = fake.settings(&path).unwrap();
        assert!(stored["user"]["data"] == metadata(&a.to_string(), Some("bridge"), Some("1")));
        assert!(stored["connection"]["autoconnect"] == OwnedValue::from(false));
        assert_eq!(fake.active_profile("br-blue0"), Some(path.clone()));

        // A second ensure changes nothing.
        assert_eq!(
            backend.ensure_bridge(a, iface("br-blue0")).await.unwrap(),
            bridge
        );
        assert_eq!(calls_then_clear(&fake), []);

        // Address: written once, reapplied each time, read back.
        backend
            .ensure_address(address(a, "br-blue0"))
            .await
            .unwrap();
        let calls = fake.calls();
        assert_eq!(calls[0].flags, Some(UPDATE_FLAGS), "Update2 flags");
        assert_eq!(
            calls_then_clear(&fake),
            [("Update2", path.clone()), ("Reapply", path.clone())]
        );
        backend
            .ensure_address(address(a, "br-blue0"))
            .await
            .unwrap();
        assert_eq!(calls_then_clear(&fake), [("Reapply", path.clone())]);
        assert_eq!(backend.addresses().await.unwrap(), [address(a, "br-blue0")]);

        // Route: the same pattern.
        backend.ensure_route(route(a, "br-blue0")).await.unwrap();
        assert_eq!(
            calls_then_clear(&fake),
            [("Update2", path.clone()), ("Reapply", path.clone())]
        );
        backend.ensure_route(route(a, "br-blue0")).await.unwrap();
        assert_eq!(calls_then_clear(&fake), [("Reapply", path.clone())]);
        assert_eq!(backend.routes().await.unwrap(), [route(a, "br-blue0")]);

        // Removal of the route and the address, then no-op repeats.
        backend.remove_route(route(a, "br-blue0")).await.unwrap();
        assert_eq!(
            calls_then_clear(&fake),
            [("Update2", path.clone()), ("Reapply", path.clone())]
        );
        assert_eq!(backend.routes().await.unwrap(), []);
        backend.remove_route(route(a, "br-blue0")).await.unwrap();
        assert_eq!(calls_then_clear(&fake), []);
        backend
            .remove_address(address(a, "br-blue0"))
            .await
            .unwrap();
        assert_eq!(
            calls_then_clear(&fake),
            [("Update2", path.clone()), ("Reapply", path.clone())]
        );
        assert_eq!(backend.addresses().await.unwrap(), []);
        backend
            .remove_address(address(a, "br-blue0"))
            .await
            .unwrap();
        assert_eq!(calls_then_clear(&fake), []);

        // Inventory reports exactly what the fake holds.
        let connections = backend.network_connections().await.unwrap();
        let owners: Vec<_> = connections
            .iter()
            .map(|value| (value.handle.as_str().to_owned(), value.owner))
            .collect();
        assert_eq!(
            owners,
            [
                (wired.clone(), None),
                (other.clone(), Some(net_b())),
                (path.clone(), Some(a))
            ]
        );
        let devices = backend.network_devices().await.unwrap();
        let summary: Vec<_> = devices
            .iter()
            .map(|device| {
                (
                    device.handle.as_str().to_owned(),
                    device.interface.as_str().to_owned(),
                    device.device_type,
                    device
                        .active_connection
                        .as_ref()
                        .map(|value| value.as_str().to_owned()),
                )
            })
            .collect();
        assert_eq!(
            summary[0],
            (
                eth0,
                "eth0".to_owned(),
                DEVICE_TYPE_ETHERNET,
                Some(wired.clone())
            )
        );
        assert_eq!(summary[1].1, "br-blue0");
        assert_eq!(
            (summary[1].2, summary[1].3.clone()),
            (DEVICE_TYPE_BRIDGE, Some(path.clone()))
        );
        assert_eq!(devices.len(), 2);

        // Remove the bridge: deactivate, then delete; a repeat does nothing.
        backend
            .remove_owned_interface(a, iface("br-blue0"))
            .await
            .unwrap();
        assert_eq!(
            calls_then_clear(&fake),
            [
                ("DeactivateConnection", path.clone()),
                ("Delete", path.clone())
            ]
        );
        assert_eq!(fake.profiles(), [wired.clone(), other.clone()]);
        backend
            .remove_owned_interface(a, iface("br-blue0"))
            .await
            .unwrap();
        assert_eq!(calls_then_clear(&fake), []);

        assert_unchanged(&fake, &untouched);
        assert_eq!(fake.active_profile("eth0"), Some(wired));
        fake.stop().await.unwrap();
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_owned_interface_profile_is_created_only_on_a_known_free_device() {
    scenario(|| async {
        let fake = FakeNetworkManager::start().await.unwrap();
        let wired = fake.add_profile(foreign("Wired", "eth0"));
        fake.add_device("eth0", DEVICE_TYPE_ETHERNET, Some(&wired));
        let bnep0 = fake.add_device("bnep0", DEVICE_TYPE_ETHERNET, None);
        let untouched = snapshot(&fake, &[&wired]);
        let backend = NetworkManagerBackend::connect_system().await.unwrap();
        let a = net_a();

        // NetworkManager does not know bnep9: refused before anything is created.
        assert_eq!(
            kind_of(
                backend.ensure_address(address(a, "bnep9")).await,
                "unknown device"
            ),
            ErrorKind::InvalidInput
        );
        assert_eq!(calls_then_clear(&fake), []);

        backend.ensure_address(address(a, "bnep0")).await.unwrap();
        let calls = fake.calls();
        let path = calls[0].profile.clone();
        assert_eq!(calls[2].device.as_deref(), Some(bnep0.as_str()));
        assert_eq!(
            calls_then_clear(&fake),
            [
                ("AddConnection2", path.clone()),
                ("Update2", path.clone()),
                ("ActivateConnection", path.clone()),
                ("Reapply", path.clone())
            ]
        );
        let stored = fake.settings(&path).unwrap();
        assert!(stored["connection"]["type"] == text("generic"));
        assert!(stored["user"]["data"] == metadata(&a.to_string(), Some("interface"), Some("1")));
        assert_eq!(backend.addresses().await.unwrap(), [address(a, "bnep0")]);

        backend
            .remove_owned_interface(a, iface("bnep0"))
            .await
            .unwrap();
        assert_eq!(
            calls_then_clear(&fake),
            [("DeactivateConnection", path.clone()), ("Delete", path)]
        );
        assert_eq!(fake.profiles(), std::slice::from_ref(&wired));
        assert_unchanged(&fake, &untouched);
        assert_eq!(fake.active_profile("eth0"), Some(wired));
        fake.stop().await.unwrap();
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_removal_is_scoped_to_the_requested_network_and_interface() {
    scenario(|| async {
        let fake = FakeNetworkManager::start().await.unwrap();
        let ours = fake.add_profile(owned(net_a(), "br-blue0", "bridge"));
        fake.add_device("br-blue0", DEVICE_TYPE_BRIDGE, Some(&ours));
        let theirs = fake.add_profile(owned(net_b(), "br-blue0", "bridge"));
        let second = fake.add_profile(owned(net_a(), "br-two", "bridge"));
        fake.add_device("br-two", DEVICE_TYPE_BRIDGE, Some(&second));
        let untouched = snapshot(&fake, &[&ours]);
        let backend = NetworkManagerBackend::connect_system().await.unwrap();

        // Removing this network's br-two deactivates and deletes only that profile.
        backend
            .remove_owned_interface(net_a(), iface("br-two"))
            .await
            .unwrap();
        assert_eq!(
            calls_then_clear(&fake),
            [("DeactivateConnection", second.clone()), ("Delete", second)]
        );

        // B's profile is inactive, so removing B's interface deletes only B's profile.
        backend
            .remove_owned_interface(net_b(), iface("br-blue0"))
            .await
            .unwrap();
        assert_eq!(calls_then_clear(&fake), [("Delete", theirs)]);
        assert_eq!(fake.profiles(), std::slice::from_ref(&ours));
        assert_eq!(fake.active_profile("br-two"), None);
        assert_eq!(fake.active_profile("br-blue0"), Some(ours));
        assert_unchanged(&fake, &untouched);
        fake.stop().await.unwrap();
    });
}

/// A foreign profile without `interface-name`: it claims no interface by name, so only the
/// device's active connection shows that the interface is in foreign use.
fn unnamed_foreign(id: &str) -> Settings {
    let mut settings = foreign(id, "unused");
    settings
        .get_mut("connection")
        .unwrap()
        .remove("interface-name");
    settings
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_owned_profiles_never_replace_a_foreign_active_connection() {
    scenario(|| async {
        let fake = FakeNetworkManager::start().await.unwrap();
        let a = net_a();
        let on_eth5 = fake.add_profile(unnamed_foreign("Unnamed on eth5"));
        fake.add_device("eth5", DEVICE_TYPE_ETHERNET, Some(&on_eth5));
        let on_bridge = fake.add_profile(unnamed_foreign("Unnamed on br-blue0"));
        fake.add_device("br-blue0", DEVICE_TYPE_BRIDGE, Some(&on_bridge));
        let ours = fake.add_profile(owned(a, "br-blue0", "bridge"));
        let generic = fake.add_profile(owned(a, "bnep0", "interface"));
        fake.add_device("bnep0", DEVICE_TYPE_ETHERNET, None);
        let untouched = snapshot(&fake, &[&on_eth5, &on_bridge, &generic]);
        let backend = NetworkManagerBackend::connect_system().await.unwrap();

        // No profile names eth5, but its device is active with a foreign connection.
        let result = backend.ensure_bridge(a, iface("eth5")).await.map(drop);
        assert_eq!(
            calls_then_clear(&fake),
            [],
            "bridge on eth5 touched NetworkManager"
        );
        assert_eq!(kind_of(result, "bridge on eth5"), ErrorKind::InvalidState);

        // This network owns br-blue0's profile, but a foreign connection is active there:
        // activation is refused instead of replacing it.
        let result = backend.ensure_bridge(a, iface("br-blue0")).await.map(drop);
        assert_eq!(
            calls_then_clear(&fake),
            [],
            "bridge activation touched NetworkManager"
        );
        assert_eq!(
            kind_of(result, "bridge activation"),
            ErrorKind::InvalidState
        );

        // Address and route are written to the owned profile first, then activation is
        // refused: the only call is `Update2` on the owned profile, never on the foreign one.
        let result = backend.ensure_address(address(a, "br-blue0")).await;
        assert_eq!(calls_then_clear(&fake), [("Update2", ours.clone())]);
        assert_eq!(
            kind_of(result, "address activation"),
            ErrorKind::InvalidState
        );
        let result = backend.ensure_route(route(a, "br-blue0")).await;
        assert_eq!(calls_then_clear(&fake), [("Update2", ours.clone())]);
        assert_eq!(kind_of(result, "route activation"), ErrorKind::InvalidState);

        // An owned non-bridge profile is not turned into a bridge.
        let result = backend.ensure_bridge(a, iface("bnep0")).await.map(drop);
        assert_eq!(calls_then_clear(&fake), [], "bridge over a generic profile");
        assert_eq!(
            kind_of(result, "bridge over a generic profile"),
            ErrorKind::InvalidState
        );

        assert_unchanged(&fake, &untouched);
        assert_eq!(fake.active_profile("eth5"), Some(on_eth5));
        assert_eq!(fake.active_profile("br-blue0"), Some(on_bridge));
        assert_eq!(fake.active_profile("bnep0"), None);
        fake.stop().await.unwrap();
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_foreign_and_other_network_claims_fail_closed() {
    scenario(|| async {
        let fake = FakeNetworkManager::start().await.unwrap();
        let wired = fake.add_profile(foreign("Wired", "eth0"));
        fake.add_device("eth0", DEVICE_TYPE_ETHERNET, Some(&wired));
        let foreign_bridge = fake.add_profile(foreign("Somebody's bridge", "br-blue0"));
        let theirs = fake.add_profile(owned(net_b(), "br-b", "bridge"));
        fake.add_device("br-b", DEVICE_TYPE_BRIDGE, Some(&theirs));
        let untouched = snapshot(&fake, &[&wired, &foreign_bridge, &theirs]);
        let backend = NetworkManagerBackend::connect_system().await.unwrap();
        let a = net_a();

        // Each refusal is checked on its own: first that NetworkManager saw no mutating call,
        // then the error kind.
        let refused = |case: &str, result: Result<(), CoreError>| {
            assert_eq!(
                calls_then_clear(&fake),
                [],
                "{case}: touched NetworkManager"
            );
            assert_eq!(kind_of(result, case), ErrorKind::InvalidState, "{case}");
        };
        refused(
            "bridge on an interface a foreign profile claims",
            backend.ensure_bridge(a, iface("br-blue0")).await.map(drop),
        );
        refused(
            "bridge on a foreign active interface",
            backend.ensure_bridge(a, iface("eth0")).await.map(drop),
        );
        refused(
            "address on a foreign active interface",
            backend.ensure_address(address(a, "eth0")).await,
        );
        refused(
            "bridge on an interface another network claims",
            backend.ensure_bridge(a, iface("br-b")).await.map(drop),
        );
        refused(
            "address on another network's active interface",
            backend.ensure_address(address(a, "br-b")).await,
        );
        refused(
            "route without a profile this network owns",
            backend.ensure_route(route(a, "br-b")).await,
        );

        // Nothing this network owns exists, so removals are no-ops that touch nothing.
        for interface in ["eth0", "br-blue0", "br-b"] {
            for (operation, result) in every_removal(&backend, a, interface).await {
                assert!(result.is_ok(), "{operation} on {interface}: {result:?}");
            }
            assert_eq!(
                calls_then_clear(&fake),
                [],
                "removals on {interface} touched NetworkManager"
            );
        }

        assert_eq!(calls_then_clear(&fake), []);
        assert_unchanged(&fake, &untouched);
        assert_eq!(fake.active_profile("eth0"), Some(wired));
        assert_eq!(fake.active_profile("br-b"), Some(theirs));
        fake.stop().await.unwrap();
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_malformed_ownership_metadata_fails_every_operation_closed() {
    scenario(|| async {
        let a = net_a();
        let owner = a.to_string();
        let variants = [
            (
                "unsupported schema",
                metadata(&owner, Some("bridge"), Some("2")),
                ErrorKind::InvalidState,
            ),
            (
                "missing schema",
                metadata(&owner, Some("bridge"), None),
                ErrorKind::InvalidState,
            ),
            (
                "missing kind",
                metadata(&owner, None, Some("1")),
                ErrorKind::InvalidState,
            ),
            (
                "unsupported kind",
                metadata(&owner, Some("tunnel"), Some("1")),
                ErrorKind::InvalidState,
            ),
            (
                "invalid owner",
                metadata("not-a-network", Some("bridge"), Some("1")),
                ErrorKind::InvalidState,
            ),
            (
                "user data not a string map",
                text("owner"),
                ErrorKind::ProtocolError,
            ),
        ];
        for (variant, user, expected) in variants {
            let fake = FakeNetworkManager::start().await.unwrap();
            let ours = fake.add_profile(owned(a, "br-blue0", "bridge"));
            fake.add_device("br-blue0", DEVICE_TYPE_BRIDGE, Some(&ours));
            let wired = fake.add_profile(foreign("Wired", "eth0"));
            let malformed = fake.add_profile(profile("Malformed", "bridge", "br-bad", Some(user)));
            let untouched = snapshot(&fake, &[&ours, &wired, &malformed]);
            let backend = NetworkManagerBackend::connect_system().await.unwrap();

            for interface in ["br-blue0", "br-bad"] {
                for (operation, result) in every_mutation(&backend, a, interface).await {
                    let case = format!("{variant}: {operation} on {interface}");
                    assert_eq!(kind_of(result, &case), expected, "{case}");
                }
            }
            let reads = [
                ("addresses", backend.addresses().await.map(drop)),
                ("routes", backend.routes().await.map(drop)),
                (
                    "network_connections",
                    backend.network_connections().await.map(drop),
                ),
            ];
            for (operation, result) in reads {
                let case = format!("{variant}: {operation}");
                assert_eq!(kind_of(result, &case), expected, "{case}");
            }

            assert_eq!(calls_then_clear(&fake), [], "{variant}");
            assert_unchanged(&fake, &untouched);
            assert_eq!(fake.active_profile("br-blue0"), Some(ours), "{variant}");
            fake.stop().await.unwrap();
        }
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_duplicate_owned_profiles_fail_closed() {
    scenario(|| async {
        let fake = FakeNetworkManager::start().await.unwrap();
        let a = net_a();
        let first = fake.add_profile(owned(a, "br-blue0", "bridge"));
        fake.add_device("br-blue0", DEVICE_TYPE_BRIDGE, Some(&first));
        let mut duplicate = owned(a, "br-blue0", "bridge");
        duplicate
            .get_mut("connection")
            .unwrap()
            .insert("uuid".to_owned(), text("uuid-duplicate"));
        let second = fake.add_profile(duplicate);
        let untouched = snapshot(&fake, &[&first, &second]);
        let backend = NetworkManagerBackend::connect_system().await.unwrap();

        for (operation, result) in every_mutation(&backend, a, "br-blue0").await {
            assert_eq!(
                kind_of(result, operation),
                ErrorKind::InvalidState,
                "{operation}"
            );
        }
        // Read-only listings still succeed.
        backend.addresses().await.unwrap();
        backend.routes().await.unwrap();
        assert_eq!(backend.network_connections().await.unwrap().len(), 2);

        assert_eq!(calls_then_clear(&fake), []);
        assert_unchanged(&fake, &untouched);
        assert_eq!(fake.active_profile("br-blue0"), Some(first));
        fake.stop().await.unwrap();
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_absent_networkmanager_is_backend_unavailable() {
    scenario(|| async {
        let unavailable = ErrorKind::NetworkBackendUnavailable;
        assert_eq!(
            kind_of(
                NetworkManagerBackend::connect_system().await.map(drop),
                "not started"
            ),
            unavailable
        );

        let fake = FakeNetworkManager::start().await.unwrap();
        let backend = NetworkManagerBackend::connect_system().await.unwrap();
        fake.stop().await.unwrap();

        assert_eq!(backend.service_owner().await.unwrap(), None);
        let a = net_a();
        let mut results = vec![
            ("version", backend.version().await.map(drop)),
            (
                "network_connections",
                backend.network_connections().await.map(drop),
            ),
            ("network_devices", backend.network_devices().await.map(drop)),
            ("addresses", backend.addresses().await.map(drop)),
            ("routes", backend.routes().await.map(drop)),
        ];
        results.extend(every_mutation(&backend, a, "br-blue0").await);
        for (operation, result) in results {
            assert_eq!(kind_of(result, operation), unavailable, "{operation}");
        }
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_scripted_method_failures_are_backend_unavailable_and_stop_the_operation() {
    scenario(|| async {
        let a = net_a();
        let unavailable = ErrorKind::NetworkBackendUnavailable;
        for method in [
            "AddConnection2",
            "ActivateConnection",
            "Update2",
            "Reapply",
            "DeactivateConnection",
            "Delete",
            "ListConnections",
            "GetSettings",
            "GetDevices",
        ] {
            let fake = FakeNetworkManager::start().await.unwrap();
            let wired = fake.add_profile(foreign("Wired", "eth0"));
            fake.add_device("eth0", DEVICE_TYPE_ETHERNET, Some(&wired));
            let theirs = fake.add_profile(owned(net_b(), "br-b", "bridge"));
            let untouched = snapshot(&fake, &[&wired, &theirs]);
            let backend = NetworkManagerBackend::connect_system().await.unwrap();

            let result = match method {
                "AddConnection2" | "ActivateConnection" => {
                    fake.fail(method);
                    backend.ensure_bridge(a, iface("br-blue0")).await.map(drop)
                }
                "Update2" | "Reapply" => {
                    backend.ensure_bridge(a, iface("br-blue0")).await.unwrap();
                    fake.clear_calls();
                    fake.fail(method);
                    backend.ensure_address(address(a, "br-blue0")).await
                }
                "DeactivateConnection" | "Delete" => {
                    backend.ensure_bridge(a, iface("br-blue0")).await.unwrap();
                    fake.clear_calls();
                    fake.fail(method);
                    backend.remove_owned_interface(a, iface("br-blue0")).await
                }
                "GetDevices" => {
                    fake.fail(method);
                    backend.network_devices().await.map(drop)
                }
                _ => {
                    fake.fail(method);
                    backend.network_connections().await.map(drop)
                }
            };
            assert_eq!(kind_of(result, method), unavailable, "{method}");

            let calls = fake.calls();
            let mutating = [
                "AddConnection2",
                "ActivateConnection",
                "Update2",
                "Reapply",
                "DeactivateConnection",
                "Delete",
            ];
            if mutating.contains(&method) {
                let last = calls.last().unwrap_or_else(|| panic!("{method}: no call"));
                assert_eq!(
                    (last.method, last.failed),
                    (method, true),
                    "{method}: {calls:#?}"
                );
                assert!(
                    calls[..calls.len() - 1].iter().all(|call| !call.failed),
                    "{method}: {calls:#?}"
                );
            } else {
                assert_eq!(calls, [], "{method}");
            }
            for call in &calls {
                assert!(
                    call.profile != wired && call.profile != theirs,
                    "{method}: touched another profile: {call:?}"
                );
            }
            assert_unchanged(&fake, &untouched);
            fake.stop().await.unwrap();
        }
    });
}

#[test]
#[ignore = "run by networkmanager_backend_scenarios_over_a_private_bus"]
fn scenario_state_subscription_reports_inventory_then_changes() {
    scenario(|| async {
        let fake = FakeNetworkManager::start().await.unwrap();
        let wired = fake.add_profile(foreign("Wired", "eth0"));
        let eth0 = fake.add_device("eth0", DEVICE_TYPE_ETHERNET, Some(&wired));
        let backend = NetworkManagerBackend::connect_system().await.unwrap();

        let mut subscription = backend.subscribe_network_state().await.unwrap();
        match within("seeded connection event", subscription.next_event())
            .await
            .unwrap()
        {
            Some(NetworkStateEvent::ConnectionAdded(value)) => {
                assert_eq!((value.handle.as_str(), value.owner), (wired.as_str(), None));
            }
            other => panic!("expected the seeded connection, got {other:?}"),
        }
        match within("seeded device event", subscription.next_event())
            .await
            .unwrap()
        {
            Some(NetworkStateEvent::DeviceAdded(value)) => {
                assert_eq!(value.handle.as_str(), eth0);
                assert_eq!(
                    value.active_connection.map(|v| v.as_str().to_owned()),
                    Some(wired)
                );
            }
            other => panic!("expected the seeded device, got {other:?}"),
        }

        let late = fake.add_profile(owned(net_a(), "br-late", "bridge"));
        match within("new connection event", subscription.next_event())
            .await
            .unwrap()
        {
            Some(NetworkStateEvent::ConnectionAdded(value)) => {
                assert_eq!(
                    (value.handle.as_str(), value.owner),
                    (late.as_str(), Some(net_a()))
                );
            }
            other => panic!("expected the new connection, got {other:?}"),
        }
        assert_eq!(calls_then_clear(&fake), []);
        fake.stop().await.unwrap();
    });
}

use std::collections::VecDeque;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use blueroute_core::{CoreError, ErrorKind, IpPrefix, NetworkId};
use blueroute_linux::{
    BackendFuture, InterfaceAddress, IpNetworkBackend, LinuxRoute, NetworkConnection,
    NetworkConnectionHandle, NetworkDevice, NetworkInterfaceHandle, NetworkManagerBackend,
    NetworkStateBackend, NetworkStateEvent, NetworkStateSubscription,
};

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn resolve<T>(mut future: BackendFuture<'_, T>) -> Result<T, CoreError> {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("contract fixture unexpectedly returned a pending future"),
    }
}

fn network(value: u8) -> NetworkId {
    NetworkId::from_bytes([value; 16])
}

fn interface(value: &str) -> NetworkInterfaceHandle {
    NetworkInterfaceHandle::new(value).expect("contract interface name is valid")
}

fn v4(address: [u8; 4], prefix_len: u8) -> IpPrefix {
    IpPrefix::new(IpAddr::V4(Ipv4Addr::from(address)), prefix_len)
        .expect("contract IP prefix is valid")
}

fn invalid_state(message: &'static str) -> CoreError {
    CoreError::new(ErrorKind::InvalidState, message)
}

#[derive(Default)]
struct FakeState {
    connections: Vec<NetworkConnection>,
    addresses: Vec<InterfaceAddress>,
    routes: Vec<LinuxRoute>,
    forwarding: bool,
}

#[derive(Default)]
struct FakeNetworkBackend {
    state: Mutex<FakeState>,
}

impl FakeNetworkBackend {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, FakeState>, CoreError> {
        self.state.lock().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::Internal,
                "fake network backend state lock failed",
                error.to_string(),
            )
        })
    }

    fn forwarding_enabled(&self) -> bool {
        self.state
            .lock()
            .expect("contract fixture lock is not poisoned")
            .forwarding
    }

    fn seed_owned_state(
        &self,
        owner: NetworkId,
        bridge: NetworkInterfaceHandle,
        address: InterfaceAddress,
        route: LinuxRoute,
    ) {
        let mut state = self
            .state
            .lock()
            .expect("contract fixture lock is not poisoned");
        state.connections.push(NetworkConnection {
            handle: NetworkConnectionHandle::new(format!("seed:{}:{}", owner, bridge.as_str()))
                .expect("seed connection handle is valid"),
            id: format!("seed-{}", bridge.as_str()),
            uuid: format!("seed-{owner}-{}", bridge.as_str()),
            connection_type: "bridge".into(),
            interface: Some(bridge),
            owner: Some(owner),
        });
        state.addresses.push(address);
        state.routes.push(route);
    }
}

struct FakeSubscription {
    pending: VecDeque<NetworkStateEvent>,
}

impl NetworkStateSubscription for FakeSubscription {
    fn next_event(&mut self) -> BackendFuture<'_, Option<NetworkStateEvent>> {
        Box::pin(async move { Ok(self.pending.pop_front()) })
    }
}

impl NetworkStateBackend for FakeNetworkBackend {
    fn network_connections(&self) -> BackendFuture<'_, Vec<NetworkConnection>> {
        Box::pin(async move {
            let mut values = self.lock()?.connections.clone();
            values.sort_by(|left, right| left.handle.cmp(&right.handle));
            Ok(values)
        })
    }

    fn network_devices(&self) -> BackendFuture<'_, Vec<NetworkDevice>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn subscribe_network_state(&self) -> BackendFuture<'_, Box<dyn NetworkStateSubscription>> {
        Box::pin(async move {
            let mut connections = self.lock()?.connections.clone();
            connections.sort_by(|left, right| left.handle.cmp(&right.handle));
            let pending = connections
                .into_iter()
                .map(NetworkStateEvent::ConnectionAdded)
                .collect();
            Ok(Box::new(FakeSubscription { pending }) as Box<dyn NetworkStateSubscription>)
        })
    }

    fn ensure_bridge(
        &self,
        owner: NetworkId,
        bridge: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, NetworkConnection> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if let Some(existing) = state
                .connections
                .iter()
                .find(|value| value.interface.as_ref() == Some(&bridge))
            {
                if existing.owner == Some(owner) {
                    return Ok(existing.clone());
                }
                return Err(invalid_state(
                    "cannot adopt an interface owned by another network",
                ));
            }

            let connection = NetworkConnection {
                handle: NetworkConnectionHandle::new(format!(
                    "fake:{}:{}",
                    owner,
                    bridge.as_str()
                ))?,
                id: format!("BlueRoute {}", bridge.as_str()),
                uuid: format!("fake-{owner}-{}", bridge.as_str()),
                connection_type: "bridge".into(),
                interface: Some(bridge),
                owner: Some(owner),
            };
            state.connections.push(connection.clone());
            Ok(connection)
        })
    }

    fn remove_owned_interface(
        &self,
        owner: NetworkId,
        interface: NetworkInterfaceHandle,
    ) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.lock()?;
            let Some(index) = state
                .connections
                .iter()
                .position(|value| value.interface.as_ref() == Some(&interface))
            else {
                return Ok(());
            };
            if state.connections[index].owner != Some(owner) {
                return Err(invalid_state(
                    "cannot remove an interface owned by another network",
                ));
            }
            state.connections.remove(index);
            state
                .addresses
                .retain(|value| !(value.owner == owner && value.interface == interface));
            state
                .routes
                .retain(|value| !(value.owner == owner && value.interface == interface));
            Ok(())
        })
    }
}

impl IpNetworkBackend for FakeNetworkBackend {
    fn addresses(&self) -> BackendFuture<'_, Vec<InterfaceAddress>> {
        Box::pin(async move {
            let mut values = self.lock()?.addresses.clone();
            values.sort_by(|left, right| {
                left.interface
                    .cmp(&right.interface)
                    .then_with(|| left.owner.cmp(&right.owner))
                    .then_with(|| left.prefix.cmp(&right.prefix))
            });
            Ok(values)
        })
    }

    fn ensure_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if state.connections.iter().any(|value| {
                value.interface.as_ref() == Some(&address.interface)
                    && value.owner != Some(address.owner)
            }) {
                return Err(invalid_state(
                    "cannot address an interface owned by another network",
                ));
            }
            if state.addresses.iter().any(|value| {
                value.interface == address.interface
                    && value.prefix == address.prefix
                    && value.owner != address.owner
            }) {
                return Err(invalid_state(
                    "cannot adopt an address owned by another network",
                ));
            }
            if !state.addresses.contains(&address) {
                state.addresses.push(address);
            }
            Ok(())
        })
    }

    fn remove_address(&self, address: InterfaceAddress) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if state.addresses.iter().any(|value| {
                value.interface == address.interface
                    && value.prefix == address.prefix
                    && value.owner != address.owner
            }) {
                return Err(invalid_state(
                    "cannot remove an address owned by another network",
                ));
            }
            state.addresses.retain(|value| value != &address);
            Ok(())
        })
    }

    fn routes(&self) -> BackendFuture<'_, Vec<LinuxRoute>> {
        Box::pin(async move {
            let mut values = self.lock()?.routes.clone();
            values.sort_by(|left, right| {
                left.interface
                    .cmp(&right.interface)
                    .then_with(|| left.owner.cmp(&right.owner))
                    .then_with(|| left.destination.cmp(&right.destination))
                    .then_with(|| left.metric.cmp(&right.metric))
            });
            Ok(values)
        })
    }

    fn ensure_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if state.connections.iter().any(|value| {
                value.interface.as_ref() == Some(&route.interface)
                    && value.owner != Some(route.owner)
            }) {
                return Err(invalid_state(
                    "cannot route through an interface owned by another network",
                ));
            }
            if state.routes.iter().any(|value| {
                value.interface == route.interface
                    && value.destination == route.destination
                    && value.owner != route.owner
            }) {
                return Err(invalid_state(
                    "cannot adopt a route owned by another network",
                ));
            }
            state.routes.retain(|value| {
                !(value.owner == route.owner
                    && value.interface == route.interface
                    && value.destination == route.destination)
            });
            state.routes.push(route);
            Ok(())
        })
    }

    fn remove_route(&self, route: LinuxRoute) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if state.routes.iter().any(|value| {
                value.interface == route.interface
                    && value.destination == route.destination
                    && value.owner != route.owner
            }) {
                return Err(invalid_state(
                    "cannot remove a route owned by another network",
                ));
            }
            state.routes.retain(|value| value != &route);
            Ok(())
        })
    }

    fn set_ipv4_forwarding(&self, enabled: bool) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.lock()?.forwarding = enabled;
            Ok(())
        })
    }
}

fn assert_network_backend_contract<B>(backend: &B)
where
    B: NetworkStateBackend + IpNetworkBackend,
{
    let owner = network(1);
    let bridge = interface("br-contract");

    let created = resolve(backend.ensure_bridge(owner, bridge.clone())).unwrap();
    let repeated = resolve(backend.ensure_bridge(owner, bridge.clone())).unwrap();
    assert_eq!(created, repeated, "bridge ensure must be idempotent");
    assert_eq!(created.owner, Some(owner));
    assert_eq!(created.interface.as_ref(), Some(&bridge));
    let matching_connections: Vec<_> = resolve(backend.network_connections())
        .unwrap()
        .into_iter()
        .filter(|value| value.interface.as_ref() == Some(&bridge))
        .collect();
    assert_eq!(matching_connections, vec![created.clone()]);

    let address = InterfaceAddress {
        interface: bridge.clone(),
        prefix: v4([10, 201, 42, 1], 24),
        owner,
    };
    resolve(backend.ensure_address(address.clone())).unwrap();
    resolve(backend.ensure_address(address.clone())).unwrap();
    let matching_addresses: Vec<_> = resolve(backend.addresses())
        .unwrap()
        .into_iter()
        .filter(|value| value.owner == owner && value.interface == bridge)
        .collect();
    assert_eq!(matching_addresses, vec![address.clone()]);

    let first_route = LinuxRoute {
        destination: v4([10, 202, 0, 0], 24),
        via: Some(IpAddr::V4(Ipv4Addr::new(10, 201, 42, 2))),
        interface: bridge.clone(),
        metric: 77,
        owner,
    };
    resolve(backend.ensure_route(first_route.clone())).unwrap();
    resolve(backend.ensure_route(first_route.clone())).unwrap();
    let updated_route = LinuxRoute {
        via: None,
        metric: 88,
        ..first_route.clone()
    };
    resolve(backend.ensure_route(updated_route.clone())).unwrap();
    let matching_routes: Vec<_> = resolve(backend.routes())
        .unwrap()
        .into_iter()
        .filter(|value| {
            value.owner == owner
                && value.interface == bridge
                && value.destination == first_route.destination
        })
        .collect();
    assert_eq!(
        matching_routes,
        vec![updated_route.clone()],
        "route ensure must reconcile stale next-hop/metric variants instead of duplicating them"
    );

    let mut subscription = resolve(backend.subscribe_network_state()).unwrap();
    assert_eq!(
        resolve(subscription.next_event()).unwrap(),
        Some(NetworkStateEvent::ConnectionAdded(created)),
        "network-state subscription must provide the current owned connection snapshot"
    );

    resolve(backend.set_ipv4_forwarding(true)).unwrap();
    resolve(backend.set_ipv4_forwarding(true)).unwrap();
    resolve(backend.set_ipv4_forwarding(false)).unwrap();
    resolve(backend.set_ipv4_forwarding(false)).unwrap();

    resolve(backend.remove_route(updated_route.clone())).unwrap();
    resolve(backend.remove_route(updated_route)).unwrap();
    assert!(
        resolve(backend.routes())
            .unwrap()
            .into_iter()
            .all(|value| !(value.owner == owner && value.interface == bridge))
    );

    resolve(backend.remove_address(address.clone())).unwrap();
    resolve(backend.remove_address(address)).unwrap();
    assert!(
        resolve(backend.addresses())
            .unwrap()
            .into_iter()
            .all(|value| !(value.owner == owner && value.interface == bridge))
    );

    resolve(backend.remove_owned_interface(owner, bridge.clone())).unwrap();
    resolve(backend.remove_owned_interface(owner, bridge.clone())).unwrap();
    assert!(
        resolve(backend.network_connections())
            .unwrap()
            .into_iter()
            .all(|value| value.interface.as_ref() != Some(&bridge))
    );
}

#[test]
fn fake_backend_passes_reusable_contract() {
    let backend = FakeNetworkBackend::default();
    assert_network_backend_contract(&backend);
    assert!(!backend.forwarding_enabled());
}

#[test]
fn fake_backend_preserves_other_owner_state_and_refuses_takeover() {
    let backend = FakeNetworkBackend::default();
    let other_owner = network(9);
    let foreign_bridge = interface("br-foreign");
    let foreign_address = InterfaceAddress {
        interface: foreign_bridge.clone(),
        prefix: v4([10, 203, 0, 1], 24),
        owner: other_owner,
    };
    let foreign_route = LinuxRoute {
        destination: v4([10, 204, 0, 0], 24),
        via: Some(IpAddr::V4(Ipv4Addr::new(10, 203, 0, 2))),
        interface: foreign_bridge.clone(),
        metric: 99,
        owner: other_owner,
    };
    backend.seed_owned_state(
        other_owner,
        foreign_bridge.clone(),
        foreign_address.clone(),
        foreign_route.clone(),
    );

    let error = resolve(backend.ensure_bridge(network(1), foreign_bridge.clone())).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidState);
    let error =
        resolve(backend.remove_owned_interface(network(1), foreign_bridge.clone())).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidState);

    assert!(
        resolve(backend.network_connections())
            .unwrap()
            .iter()
            .any(|value| value.owner == Some(other_owner)
                && value.interface.as_ref() == Some(&foreign_bridge))
    );
    assert!(
        resolve(backend.addresses())
            .unwrap()
            .contains(&foreign_address)
    );
    assert!(resolve(backend.routes()).unwrap().contains(&foreign_route));
}

#[test]
fn networkmanager_implements_the_same_backend_contract_traits() {
    fn assert_conforms<T>()
    where
        T: NetworkStateBackend + IpNetworkBackend,
    {
    }

    assert_conforms::<NetworkManagerBackend>();
}

#[test]
fn core_and_topology_remain_free_of_networkmanager_implementation_types() {
    let core_manifest = include_str!("../../blueroute-core/Cargo.toml");
    assert!(!core_manifest.contains("blueroute-linux"));
    assert!(!core_manifest.contains("zbus"));

    let core_lib = include_str!("../../blueroute-core/src/lib.rs");
    let topology = include_str!("../../blueroute-core/src/topology.rs");
    for source in [core_lib, topology] {
        assert!(!source.contains("NetworkManagerBackend"));
        assert!(!source.contains("org.freedesktop.NetworkManager"));
        assert!(!source.contains("zbus::"));
        assert!(!source.contains("blueroute_linux::"));
    }
}

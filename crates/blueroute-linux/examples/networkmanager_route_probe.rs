use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind, IpPrefix, NetworkId};
use blueroute_linux::{
    InterfaceAddress, IpNetworkBackend, LinuxRoute, NetworkConnection, NetworkInterfaceHandle,
    NetworkManagerBackend, NetworkStateBackend,
};

const PROBE_OWNER: NetworkId = NetworkId::from_bytes([0x49; 16]);
const OTHER_OWNER: NetworkId = NetworkId::from_bytes([0x4a; 16]);
const PROBE_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 254, 90, 1));
const PROBE_PREFIX_LEN: u8 = 30;
const ROUTE_DESTINATION: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 254, 91, 0));
const ROUTE_PREFIX_LEN: u8 = 24;
const ROUTE_NEXT_HOP: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 254, 90, 2));
const INITIAL_METRIC: u32 = 177;
const FINAL_METRIC: u32 = 77;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let bridge_name = env::args()
        .nth(1)
        .unwrap_or_else(|| "br-blue-rt".to_owned());
    let hold_seconds = env::args()
        .nth(2)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(60);
    let bridge = NetworkInterfaceHandle::new(bridge_name)?;
    let address = InterfaceAddress {
        interface: bridge.clone(),
        prefix: IpPrefix::new(PROBE_ADDRESS, PROBE_PREFIX_LEN)?,
        owner: PROBE_OWNER,
    };
    let destination = IpPrefix::new(ROUTE_DESTINATION, ROUTE_PREFIX_LEN)?;
    let initial_route = LinuxRoute {
        destination,
        via: Some(ROUTE_NEXT_HOP),
        interface: bridge.clone(),
        metric: INITIAL_METRIC,
        owner: PROBE_OWNER,
    };
    let route = LinuxRoute {
        metric: FINAL_METRIC,
        ..initial_route.clone()
    };

    let backend = NetworkManagerBackend::connect_system().await?;
    println!("NetworkManager version: {}", backend.version().await?);

    // A previous interrupted probe may have left state belonging to this fixed owner.
    // Cleanup is owner-scoped and deliberately repeated to prove idempotence.
    backend
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;
    backend
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;

    let baseline_connections = backend.network_connections().await?;
    let foreign_profiles: Vec<NetworkConnection> = baseline_connections
        .iter()
        .filter(|profile| profile.owner.is_none())
        .cloned()
        .collect();
    println!(
        "baseline: connections={} foreign-profiles={}",
        baseline_connections.len(),
        foreign_profiles.len()
    );

    let profile = backend.ensure_bridge(PROBE_OWNER, bridge.clone()).await?;
    backend.ensure_address(address.clone()).await?;
    println!(
        "route test interface ready: interface={} profile={} address={}/{}",
        bridge.as_str(),
        profile.handle.as_str(),
        PROBE_ADDRESS,
        PROBE_PREFIX_LEN
    );

    backend.ensure_route(initial_route.clone()).await?;
    backend.ensure_route(route.clone()).await?;
    backend.ensure_route(route.clone()).await?;
    assert_exact_route(&backend, &route, &initial_route).await?;
    println!(
        "route ready: destination={}/{} via={} metric={} repeated-ensure=single-route update-from-metric={}",
        ROUTE_DESTINATION,
        ROUTE_PREFIX_LEN,
        ROUTE_NEXT_HOP,
        FINAL_METRIC,
        INITIAL_METRIC
    );

    // Reconnect to NetworkManager through a fresh backend instance. P4-008 must rediscover
    // durable route state rather than depend on a remembered successful method call.
    let reconnected = NetworkManagerBackend::connect_system().await?;
    assert_exact_route(&reconnected, &route, &initial_route).await?;
    reconnected.ensure_route(route.clone()).await?;
    assert_exact_route(&reconnected, &route, &initial_route).await?;
    println!("route rediscovered and reconciled after fresh backend connection");

    let other_route = LinuxRoute {
        owner: OTHER_OWNER,
        ..route.clone()
    };
    match reconnected.ensure_route(other_route.clone()).await {
        Err(error) if error.kind() == ErrorKind::InvalidState => {
            println!("cross-owner route takeover rejected: {error}");
        }
        Err(error) => return Err(error.into()),
        Ok(()) => {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "a second BlueRoute owner unexpectedly modified the route",
            )
            .into());
        }
    }
    if reconnected
        .routes()
        .await?
        .iter()
        .any(|current| current.owner == OTHER_OWNER && current.interface == bridge)
    {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "rejected cross-owner route operation leaked route state",
        )
        .into());
    }
    println!("cross-owner route rejection left no leaked state");

    reconnected.remove_route(other_route).await?;
    assert_exact_route(&reconnected, &route, &initial_route).await?;
    println!("wrong-owner route cleanup was a safe no-op");

    assert_foreign_profiles_preserved(
        &foreign_profiles,
        &reconnected.network_connections().await?,
    )?;
    println!("foreign NetworkManager profiles preserved before hold");

    println!(
        "holding configured route for {hold_seconds}s; verify in another terminal with: ip -4 route show {}/{}",
        ROUTE_DESTINATION, ROUTE_PREFIX_LEN
    );
    Timer::after(Duration::from_secs(hold_seconds)).await;

    reconnected.remove_route(route.clone()).await?;
    reconnected.remove_route(route.clone()).await?;
    if reconnected.routes().await?.contains(&route) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "BlueRoute-owned route remains after repeated remove_route",
        )
        .into());
    }
    println!("route removed; repeated remove succeeded");

    reconnected.remove_address(address.clone()).await?;
    reconnected.remove_address(address).await?;
    reconnected
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;
    reconnected
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;

    let final_connections = reconnected.network_connections().await?;
    assert_foreign_profiles_preserved(&foreign_profiles, &final_connections)?;
    if final_connections.iter().any(|profile| {
        profile.owner == Some(PROBE_OWNER) && profile.interface.as_ref() == Some(&bridge)
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "BlueRoute-owned route probe profile remains after cleanup",
        )
        .into());
    }
    println!("bridge/profile removed; repeated cleanup succeeded");
    println!("foreign NetworkManager profiles preserved after cleanup");
    println!("P4-008 NetworkManager route probe PASS");
    Ok(())
}

async fn assert_exact_route(
    backend: &NetworkManagerBackend,
    expected: &LinuxRoute,
    stale: &LinuxRoute,
) -> Result<(), CoreError> {
    let routes = backend.routes().await?;
    let same_destination: Vec<_> = routes
        .iter()
        .filter(|current| {
            current.owner == expected.owner
                && current.interface == expected.interface
                && current.destination == expected.destination
        })
        .collect();
    if same_destination.len() != 1 || same_destination[0] != expected {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "NetworkManager route reconciliation did not produce exactly the desired route",
            format!("matching-routes={same_destination:?}"),
        ));
    }
    if routes.contains(stale) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "stale route variant remains after route update reconciliation",
        ));
    }
    Ok(())
}

fn assert_foreign_profiles_preserved(
    before: &[NetworkConnection],
    after: &[NetworkConnection],
) -> Result<(), CoreError> {
    for original in before {
        let Some(current) = after
            .iter()
            .find(|profile| profile.handle == original.handle)
        else {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "NetworkManager profile present before the route probe disappeared",
                format!("profile={} id={}", original.handle.as_str(), original.id),
            ));
        };
        if current.id != original.id || current.uuid != original.uuid || current.owner.is_some() {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "foreign NetworkManager profile changed ownership or identity during route probe",
                format!("profile={} id={}", original.handle.as_str(), original.id),
            ));
        }
    }
    Ok(())
}

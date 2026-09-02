use std::env;
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_core::{
    CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId,
    ensure_ipv4_segment_available,
};
use blueroute_linux::{
    BluetoothBackend, BluezBackend, InterfaceAddress, IpNetworkBackend,
    IpNetworkObservationBackend, NetworkInterfaceHandle, NetworkManagerBackend,
    NetworkStateBackend, PanBackend,
};
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, Proxy};

const NETWORKMANAGER_DEVICE_WAIT: Duration = Duration::from_secs(8);
const NETWORKMANAGER_POLL_INTERVAL: Duration = Duration::from_millis(250);

fn main() {
    if let Err(error) = futures_lite::future::block_on(run()) {
        eprintln!("P6-006 client setup failed: {}", error.message());
        if let Some(diagnostic) = error.diagnostic() {
            eprintln!("diagnostic: {diagnostic}");
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<(), CoreError> {
    let (target_name, network, hold_seconds) = parse_arguments()?;
    let plan = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default())?;

    let network_backend = NetworkManagerBackend::connect_system().await?;
    let active = network_backend.active_ipv4_prefixes().await?;
    if let Err(error) = ensure_ipv4_segment_available(plan.segment, active) {
        return Err(CoreError::with_diagnostic(
            error.kind(),
            error.message(),
            format!(
                "network={network} segment={}/{}; {}",
                plan.segment.address,
                plan.segment.prefix_len,
                error.diagnostic().unwrap_or("no conflict diagnostic")
            ),
        ));
    }

    let bluez = BluezBackend::connect_system().await?;
    let adapter = bluez
        .adapters()
        .await?
        .into_iter()
        .find(|adapter| adapter.powered)
        .ok_or_else(|| {
            CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "no powered Bluetooth adapter is available",
            )
        })?;

    bluez.start_discovery(adapter.handle.clone()).await?;
    Timer::after(Duration::from_secs(10)).await;
    let peers = bluez.discovered_peers(adapter.handle.clone()).await;
    let stop_discovery = bluez.stop_discovery(adapter.handle).await;
    let peers = peers?;
    stop_discovery?;
    let peer = peers
        .into_iter()
        .find(|peer| peer.display_name.as_deref() == Some(target_name.as_str()))
        .ok_or_else(|| {
            CoreError::with_diagnostic(
                ErrorKind::CapabilityUnavailable,
                "requested Bluetooth peer was not discovered",
                format!("peer_name={target_name}"),
            )
        })?;

    let attachment = bluez.connect_panu(peer.handle.clone()).await?;
    if let Err(error) =
        wait_for_networkmanager_device(&network_backend, &attachment.interface).await
    {
        let disconnect = bluez.disconnect_panu(peer.handle.clone()).await;
        return Err(with_cleanup(error, [Ok(()), disconnect]));
    }
    let address = InterfaceAddress {
        interface: attachment.interface.clone(),
        prefix: plan.first_client,
        owner: network,
    };
    if let Err(error) = network_backend.ensure_address(address.clone()).await {
        let network_cleanup = cleanup_network(&network_backend, &address).await;
        let disconnect = bluez.disconnect_panu(peer.handle.clone()).await;
        return Err(with_cleanup(error, [network_cleanup, disconnect]));
    }

    println!("P6-006 client ready");
    println!("network={network}");
    println!(
        "segment={}/{} host={}/{} client={}/{} interface={}",
        plan.segment.address,
        plan.segment.prefix_len,
        plan.host.address,
        plan.host.prefix_len,
        plan.first_client.address,
        plan.first_client.prefix_len,
        attachment.interface.as_str()
    );
    println!("hold={hold_seconds}s");
    println!(
        "Use ordinary applications with raw host IPv4 address {} while this probe is holding.",
        plan.host.address
    );

    Timer::after(Duration::from_secs(hold_seconds)).await;

    let network_cleanup = cleanup_network(&network_backend, &address).await;
    let first_disconnect = bluez.disconnect_panu(peer.handle.clone()).await;
    let second_disconnect = bluez.disconnect_panu(peer.handle).await;
    combine_cleanup([network_cleanup, first_disconnect, second_disconnect])?;
    println!("P6-006 client cleanup PASS");
    Ok(())
}

fn parse_arguments() -> Result<(String, NetworkId, u64), CoreError> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let usage = "usage: single_star_traffic_client <peer-name> <network-id> [hold-seconds]";
    match arguments.as_slice() {
        [target_name, network] => Ok((target_name.clone(), network.parse()?, 300)),
        [target_name, network, hold_seconds] => Ok((
            target_name.clone(),
            network.parse()?,
            parse_hold_seconds(hold_seconds)?,
        )),
        _ => Err(CoreError::new(ErrorKind::InvalidInput, usage)),
    }
}

fn parse_hold_seconds(value: &str) -> Result<u64, CoreError> {
    value.parse::<u64>().map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::InvalidInput,
            "hold-seconds must be an unsigned integer",
            error.to_string(),
        )
    })
}

async fn wait_for_networkmanager_device(
    backend: &NetworkManagerBackend,
    interface: &NetworkInterfaceHandle,
) -> Result<(), CoreError> {
    let deadline = Instant::now() + NETWORKMANAGER_DEVICE_WAIT;
    loop {
        if backend
            .network_devices()
            .await?
            .iter()
            .any(|device| device.interface == *interface)
        {
            return Ok(());
        }

        if let Some(observation) = networkmanager_ip_interface_observation(interface).await? {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "NetworkManager exposes the PANU only as an IP data interface",
                observation,
            ));
        }

        if Instant::now() >= deadline {
            let observed = networkmanager_device_snapshot().await?;
            return Err(CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                "NetworkManager did not observe the PANU interface before the timeout",
                format!(
                    "interface={} observed_devices=[{}]",
                    interface.as_str(),
                    observed
                ),
            ));
        }
        Timer::after(NETWORKMANAGER_POLL_INTERVAL).await;
    }
}

async fn networkmanager_ip_interface_observation(
    interface: &NetworkInterfaceHandle,
) -> Result<Option<String>, CoreError> {
    let observations = networkmanager_device_observations().await?;
    Ok(observations
        .into_iter()
        .find(|observation| observation.ip_interface.as_deref() == Some(interface.as_str()))
        .map(|observation| observation.describe()))
}

async fn networkmanager_device_snapshot() -> Result<String, CoreError> {
    let observations = networkmanager_device_observations().await?;
    if observations.is_empty() {
        Ok("<none>".to_owned())
    } else {
        Ok(observations
            .into_iter()
            .map(|observation| observation.describe())
            .collect::<Vec<_>>()
            .join(" | "))
    }
}

struct NetworkManagerDeviceObservation {
    control_interface: String,
    ip_interface: Option<String>,
    managed: bool,
    device_type: u32,
    state: u32,
}

impl NetworkManagerDeviceObservation {
    fn describe(&self) -> String {
        format!(
            "control={} ip={} managed={} type={} state={}",
            self.control_interface,
            self.ip_interface.as_deref().unwrap_or("<none>"),
            self.managed,
            self.device_type,
            self.state
        )
    }
}

async fn networkmanager_device_observations()
-> Result<Vec<NetworkManagerDeviceObservation>, CoreError> {
    const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
    const NM_PATH: &str = "/org/freedesktop/NetworkManager";
    const NM_INTERFACE: &str = "org.freedesktop.NetworkManager";
    const NM_DEVICE_INTERFACE: &str = "org.freedesktop.NetworkManager.Device";

    let connection = Connection::system().await.map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to connect to system D-Bus for NetworkManager diagnostics",
            error.to_string(),
        )
    })?;
    let manager = Proxy::new(&connection, NM_SERVICE, NM_PATH, NM_INTERFACE)
        .await
        .map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::NetworkBackendUnavailable,
                "failed to create NetworkManager diagnostic proxy",
                error.to_string(),
            )
        })?;
    let paths: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await.map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to enumerate NetworkManager devices for diagnostics",
            error.to_string(),
        )
    })?;

    let mut observations = Vec::with_capacity(paths.len());
    for path in paths {
        let proxy = Proxy::new(&connection, NM_SERVICE, path.as_str(), NM_DEVICE_INTERFACE)
            .await
            .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::NetworkBackendUnavailable,
                    "failed to inspect a NetworkManager device",
                    error.to_string(),
                )
            })?;
        let control_interface: String = proxy
            .get_property("Interface")
            .await
            .map_err(|error| diagnostic_property_error("Interface", error))?;
        let ip_interface: String = proxy
            .get_property("IpInterface")
            .await
            .map_err(|error| diagnostic_property_error("IpInterface", error))?;
        let managed: bool = proxy
            .get_property("Managed")
            .await
            .map_err(|error| diagnostic_property_error("Managed", error))?;
        let device_type: u32 = proxy
            .get_property("DeviceType")
            .await
            .map_err(|error| diagnostic_property_error("DeviceType", error))?;
        let state: u32 = proxy
            .get_property("State")
            .await
            .map_err(|error| diagnostic_property_error("State", error))?;
        observations.push(NetworkManagerDeviceObservation {
            control_interface,
            ip_interface: (!ip_interface.is_empty()).then_some(ip_interface),
            managed,
            device_type,
            state,
        });
    }
    Ok(observations)
}

fn diagnostic_property_error(property: &str, error: zbus::Error) -> CoreError {
    CoreError::with_diagnostic(
        ErrorKind::NetworkBackendUnavailable,
        "failed to read a NetworkManager diagnostic property",
        format!("property={property}: {error}"),
    )
}

async fn cleanup_network(
    backend: &NetworkManagerBackend,
    address: &InterfaceAddress,
) -> Result<(), CoreError> {
    let remove_address = backend.remove_address(address.clone()).await;
    let remove_profile = backend
        .remove_owned_interface(address.owner, address.interface.clone())
        .await;
    combine_cleanup([remove_address, remove_profile])
}

fn combine_cleanup<const N: usize>(results: [Result<(), CoreError>; N]) -> Result<(), CoreError> {
    let failures: Vec<String> = results
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.to_string())
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "P6-006 client cleanup did not fully converge",
            failures.join("; "),
        ))
    }
}

fn with_cleanup<const N: usize>(
    primary: CoreError,
    cleanup: [Result<(), CoreError>; N],
) -> CoreError {
    match combine_cleanup(cleanup) {
        Ok(()) => primary,
        Err(cleanup) => CoreError::with_diagnostic(
            primary.kind(),
            primary.message(),
            format!(
                "{}; cleanup also failed: {cleanup}",
                primary.diagnostic().unwrap_or("no primary diagnostic")
            ),
        ),
    }
}

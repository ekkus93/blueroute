use std::env;
use std::str::FromStr;
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{
    CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId,
    ensure_ipv4_segment_available,
};
use blueroute_linux::{
    BluetoothBackend, BluezBackend, InterfaceAddress, IpNetworkBackend,
    IpNetworkObservationBackend, NetworkManagerBackend, NetworkStateBackend, PanBackend,
};

const DEFAULT_NETWORK: NetworkId = NetworkId::from_bytes([0x66; 16]);

fn main() {
    if let Err(error) = futures_lite::future::block_on(run()) {
        eprintln!("P6-006 client setup failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let target_name = env::args().nth(1).ok_or(
        "usage: cargo run -p blueroute-linux --example single_star_traffic_client --locked -- <peer-name> [network-id] [hold-seconds]",
    )?;
    let network = env::args()
        .nth(2)
        .map(|value| NetworkId::from_str(&value))
        .transpose()?
        .unwrap_or(DEFAULT_NETWORK);
    let hold_seconds = env::args()
        .nth(3)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(300);
    let plan = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default())?;

    let network_backend = NetworkManagerBackend::connect_system().await?;
    let active = network_backend.active_ipv4_prefixes().await?;
    ensure_ipv4_segment_available(plan.segment, active)?;

    let bluez = BluezBackend::connect_system().await?;
    let adapter = bluez
        .adapters()
        .await?
        .into_iter()
        .find(|adapter| adapter.powered)
        .ok_or("no powered Bluetooth adapter is available")?;

    bluez.start_discovery(adapter.handle.clone()).await?;
    Timer::after(Duration::from_secs(10)).await;
    let peers = bluez.discovered_peers(adapter.handle.clone()).await;
    let stop_discovery = bluez.stop_discovery(adapter.handle).await;
    let peers = peers?;
    stop_discovery?;
    let peer = peers
        .into_iter()
        .find(|peer| peer.display_name.as_deref() == Some(target_name.as_str()))
        .ok_or_else(|| format!("Bluetooth peer {target_name:?} was not discovered"))?;

    let attachment = bluez.connect_panu(peer.handle.clone()).await?;
    let address = InterfaceAddress {
        interface: attachment.interface.clone(),
        prefix: plan.first_client,
        owner: network,
    };
    if let Err(error) = network_backend.ensure_address(address.clone()).await {
        let network_cleanup = cleanup_network(&network_backend, &address).await;
        let disconnect = bluez.disconnect_panu(peer.handle.clone()).await;
        return Err(with_cleanup(error, [network_cleanup, disconnect]).into());
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

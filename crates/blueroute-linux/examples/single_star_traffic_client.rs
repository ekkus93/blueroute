use std::env;
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{
    CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId,
    ensure_ipv4_segment_available,
};
use blueroute_linux::{
    BluetoothBackend, BluezBackend, InterfaceAddress, IpNetworkObservationBackend,
    KernelAddressBackend, NetworkManagerBackend, PanBackend,
};

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
    let address = InterfaceAddress {
        interface: attachment.interface.clone(),
        prefix: plan.first_client,
        owner: network,
    };
    let kernel_backend = KernelAddressBackend;
    let lease = match kernel_backend.ensure_panu_address(address) {
        Ok(lease) => lease,
        Err(error) => {
            let disconnect = bluez.disconnect_panu(peer.handle.clone()).await;
            return Err(with_cleanup(error, [disconnect]));
        }
    };

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

    let address_cleanup = kernel_backend.remove_panu_address(lease);
    let first_disconnect = bluez.disconnect_panu(peer.handle.clone()).await;
    let second_disconnect = bluez.disconnect_panu(peer.handle).await;
    combine_cleanup([address_cleanup, first_disconnect, second_disconnect])?;
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

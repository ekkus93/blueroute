use std::env;
use std::fs::File;
use std::io::Read;
use std::time::Duration;

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

const MAX_NETWORK_ATTEMPTS: usize = 16;

fn main() {
    if let Err(error) = futures_lite::future::block_on(run()) {
        eprintln!("P6-006 host setup failed: {}", error.message());
        if let Some(diagnostic) = error.diagnostic() {
            eprintln!("diagnostic: {diagnostic}");
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<(), CoreError> {
    let (requested_network, hold_seconds) = parse_arguments()?;
    let network_backend = NetworkManagerBackend::connect_system().await?;
    let (network, plan) = select_network(&network_backend, requested_network).await?;
    let bridge = NetworkInterfaceHandle::new(format!("brp6-{}", &network.to_string()[..8]))?;
    let address = InterfaceAddress {
        interface: bridge.clone(),
        prefix: plan.host,
        owner: network,
    };

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

    network_backend
        .ensure_bridge(network, bridge.clone())
        .await?;
    if let Err(error) = network_backend.ensure_address(address.clone()).await {
        let cleanup = cleanup_network(&network_backend, &address).await;
        return Err(with_cleanup(error, cleanup));
    }

    let nap = match bluez
        .start_nap(adapter.handle.clone(), bridge.clone())
        .await
    {
        Ok(nap) => nap,
        Err(error) => {
            let cleanup = cleanup_network(&network_backend, &address).await;
            return Err(with_cleanup(error, cleanup));
        }
    };

    println!("P6-006 host ready");
    println!("network={network}");
    println!(
        "segment={}/{} host={}/{} client={}/{} bridge={}",
        plan.segment.address,
        plan.segment.prefix_len,
        plan.host.address,
        plan.host.prefix_len,
        plan.first_client.address,
        plan.first_client.prefix_len,
        nap.interface.as_str()
    );
    println!("hold={hold_seconds}s");
    println!(
        "Run ordinary applications against host raw IPv4 address {} while this probe is holding.",
        plan.host.address
    );

    Timer::after(Duration::from_secs(hold_seconds)).await;

    let first_stop = bluez.stop_nap(adapter.handle.clone()).await;
    let second_stop = bluez.stop_nap(adapter.handle).await;
    let network_cleanup = cleanup_network(&network_backend, &address).await;
    combine_cleanup([first_stop, second_stop, network_cleanup])?;
    println!("P6-006 host cleanup PASS");
    Ok(())
}

fn parse_arguments() -> Result<(Option<NetworkId>, u64), CoreError> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let usage = "usage: single_star_traffic_host [auto|<network-id>] [hold-seconds]";
    match arguments.as_slice() {
        [] => Ok((None, 300)),
        [network] if network == "auto" => Ok((None, 300)),
        [network] => Ok((Some(parse_network(network)?), 300)),
        [network, hold_seconds] => Ok((
            if network == "auto" {
                None
            } else {
                Some(parse_network(network)?)
            },
            parse_hold_seconds(hold_seconds)?,
        )),
        _ => Err(CoreError::new(ErrorKind::InvalidInput, usage)),
    }
}

fn parse_network(value: &str) -> Result<NetworkId, CoreError> {
    value.parse::<NetworkId>()
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

async fn select_network(
    backend: &NetworkManagerBackend,
    requested: Option<NetworkId>,
) -> Result<(NetworkId, Ipv4StarAddressPlan), CoreError> {
    let active = backend.active_ipv4_prefixes().await?;
    if let Some(network) = requested {
        let plan = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default())?;
        return match ensure_ipv4_segment_available(plan.segment, active) {
            Ok(()) => Ok((network, plan)),
            Err(error) => Err(CoreError::with_diagnostic(
                error.kind(),
                error.message(),
                format!(
                    "requested_network={network} requested_segment={}/{}; {}",
                    plan.segment.address,
                    plan.segment.prefix_len,
                    error.diagnostic().unwrap_or("no conflict diagnostic")
                ),
            )),
        };
    }

    let mut random = File::open("/dev/urandom").map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::PersistenceError,
            "failed to open the Linux random source for a P6-006 network identity",
            error.to_string(),
        )
    })?;
    let mut last_conflict = None;
    for attempt in 1..=MAX_NETWORK_ATTEMPTS {
        let mut bytes = [0_u8; 16];
        random.read_exact(&mut bytes).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PersistenceError,
                "failed to generate a P6-006 network identity",
                error.to_string(),
            )
        })?;
        let network = NetworkId::from_bytes(bytes);
        let plan = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default())?;
        match ensure_ipv4_segment_available(plan.segment, active.iter().copied()) {
            Ok(()) => {
                println!(
                    "P6-006 auto-selected conflict-free network on attempt {attempt}: network={network} segment={}/{}",
                    plan.segment.address, plan.segment.prefix_len
                );
                return Ok((network, plan));
            }
            Err(error) if error.kind() == ErrorKind::AddressConflict => {
                eprintln!(
                    "P6-006 rejected conflicting candidate: network={network} segment={}/{} diagnostic={}",
                    plan.segment.address,
                    plan.segment.prefix_len,
                    error.diagnostic().unwrap_or("no conflict diagnostic")
                );
                last_conflict = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(CoreError::with_diagnostic(
        ErrorKind::AddressConflict,
        "failed to select a conflict-free P6-006 IPv4 segment",
        format!(
            "attempted {MAX_NETWORK_ATTEMPTS} random network identities; last conflict: {}",
            last_conflict
                .as_ref()
                .and_then(CoreError::diagnostic)
                .unwrap_or("no conflict diagnostic")
        ),
    ))
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
            "P6-006 host cleanup did not fully converge",
            failures.join("; "),
        ))
    }
}

fn with_cleanup(primary: CoreError, cleanup: Result<(), CoreError>) -> CoreError {
    match cleanup {
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

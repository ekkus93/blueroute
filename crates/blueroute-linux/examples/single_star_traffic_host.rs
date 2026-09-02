use std::env;
use std::str::FromStr;
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{
    CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId,
    ensure_ipv4_segment_available,
};
use blueroute_linux::{
    BluetoothBackend, BluezBackend, InterfaceAddress, IpNetworkBackend, IpNetworkObservationBackend,
    NetworkInterfaceHandle, NetworkManagerBackend, NetworkStateBackend, PanBackend,
};

const DEFAULT_NETWORK: NetworkId = NetworkId::from_bytes([0x66; 16]);

fn main() {
    if let Err(error) = futures_lite::future::block_on(run()) {
        eprintln!("P6-006 host setup failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let network = env::args()
        .nth(1)
        .map(|value| NetworkId::from_str(&value))
        .transpose()?
        .unwrap_or(DEFAULT_NETWORK);
    let hold_seconds = env::args()
        .nth(2)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(300);
    let plan = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default())?;
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
        .ok_or("no powered Bluetooth adapter is available")?;
    let network_backend = NetworkManagerBackend::connect_system().await?;
    let active = network_backend.active_ipv4_prefixes().await?;
    ensure_ipv4_segment_available(plan.segment, active)?;

    network_backend
        .ensure_bridge(network, bridge.clone())
        .await?;
    if let Err(error) = network_backend.ensure_address(address.clone()).await {
        let cleanup = cleanup_network(&network_backend, &address).await;
        return Err(with_cleanup(error, cleanup).into());
    }

    let nap = match bluez.start_nap(adapter.handle.clone(), bridge.clone()).await {
        Ok(nap) => nap,
        Err(error) => {
            let cleanup = cleanup_network(&network_backend, &address).await;
            return Err(with_cleanup(error, cleanup).into());
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

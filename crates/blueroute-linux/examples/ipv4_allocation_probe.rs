use std::error::Error;
use std::str::FromStr;

use blueroute_core::{
    CoreError, ErrorKind, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId,
    ensure_ipv4_segment_available,
};
use blueroute_linux::{
    InterfaceAddress, IpNetworkBackend, IpNetworkObservationBackend, NetworkInterfaceHandle,
    NetworkManagerBackend, NetworkStateBackend,
};
use futures_lite::future;

fn main() {
    if let Err(error) = future::block_on(run()) {
        eprintln!("P6-005 IPv4 allocation probe failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let network = std::env::args()
        .nth(1)
        .map(|value| NetworkId::from_str(&value))
        .transpose()?
        .unwrap_or_else(|| NetworkId::from_bytes([0x65; 16]));
    let plan = Ipv4StarAddressPlan::for_network(network, Ipv4AddressPool::default())?;
    let short = &network.to_string()[..8];
    let interface = NetworkInterfaceHandle::new(format!("brp-{short}"))?;
    let address = InterfaceAddress {
        interface: interface.clone(),
        prefix: plan.host,
        owner: network,
    };
    let backend = NetworkManagerBackend::connect_system().await?;

    println!("network={network}");
    println!(
        "segment={}/{} host={}/{} first_client={}/{}",
        plan.segment.address,
        plan.segment.prefix_len,
        plan.host.address,
        plan.host.prefix_len,
        plan.first_client.address,
        plan.first_client.prefix_len
    );

    for cycle in 1..=2 {
        let active = backend.active_ipv4_prefixes().await?;
        ensure_ipv4_segment_available(plan.segment, active)?;

        backend.ensure_bridge(network, interface.clone()).await?;
        if let Err(error) = backend.ensure_address(address.clone()).await {
            let cleanup = backend
                .remove_owned_interface(network, interface.clone())
                .await;
            return match cleanup {
                Ok(()) => Err(error.into()),
                Err(cleanup_error) => Err(format!(
                    "cycle {cycle}: address apply failed: {error}; bridge cleanup also failed: {cleanup_error}"
                )
                .into()),
            };
        }

        let verification = verify_applied(&backend, &address, cycle).await;
        let cleanup = cleanup_cycle(&backend, &address).await;
        match (verification, cleanup) {
            (Ok(()), Ok(())) => {}
            (Err(error), Ok(())) => return Err(error.into()),
            (Ok(()), Err(error)) => return Err(error.into()),
            (Err(error), Err(cleanup_error)) => {
                return Err(format!(
                    "cycle {cycle}: verification failed: {error}; cleanup also failed: {cleanup_error}"
                )
                .into());
            }
        }

        if backend.addresses().await?.contains(&address) {
            return Err(format!("cycle {cycle}: host address remained after cleanup").into());
        }
        if backend
            .network_connections()
            .await?
            .iter()
            .any(|connection| {
                connection.owner == Some(network)
                    && connection.interface.as_ref() == Some(&interface)
            })
        {
            return Err(
                format!("cycle {cycle}: BlueRoute-owned profile remained after cleanup").into(),
            );
        }
        let active = backend.active_ipv4_prefixes().await?;
        ensure_ipv4_segment_available(plan.segment, active)?;
        println!("cycle={cycle} cleanup=clean");
    }

    println!("P6-005 IPv4 allocation probe PASS");
    Ok(())
}

async fn verify_applied(
    backend: &NetworkManagerBackend,
    address: &InterfaceAddress,
    cycle: usize,
) -> Result<(), CoreError> {
    if backend.addresses().await?.contains(address) {
        println!(
            "cycle={cycle} applied={}/{}",
            address.prefix.address, address.prefix.prefix_len
        );
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorKind::InvalidState,
            format!("cycle {cycle}: NetworkManager did not report the applied host address"),
        ))
    }
}

async fn cleanup_cycle(
    backend: &NetworkManagerBackend,
    address: &InterfaceAddress,
) -> Result<(), CoreError> {
    let remove_address = backend.remove_address(address.clone()).await;
    let remove_profile = backend
        .remove_owned_interface(address.owner, address.interface.clone())
        .await;
    match (remove_address, remove_profile) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(profile_error)) => Err(CoreError::with_diagnostic(
            error.kind(),
            error.message(),
            format!(
                "{}; profile cleanup also failed: {profile_error}",
                error
                    .diagnostic()
                    .unwrap_or("no address cleanup diagnostic")
            ),
        )),
    }
}

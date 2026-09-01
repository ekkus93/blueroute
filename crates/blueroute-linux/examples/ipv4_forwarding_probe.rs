use std::env;
use std::fs;
use std::path::Path;
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind};
use blueroute_linux::{IpNetworkBackend, NetworkManagerBackend};

const IPV4_FORWARD_SYSCTL: &str = "/proc/sys/net/ipv4/ip_forward";
const LEASE_PATH: &str = "/run/blueroute/ipv4-forwarding-v1.state";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let hold_seconds = env::args()
        .nth(1)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(90);

    let backend = NetworkManagerBackend::connect_system().await?;
    println!("NetworkManager version: {}", backend.version().await?);

    // A prior interrupted acceptance probe may have left a BlueRoute runtime lease. Releasing is
    // safe to repeat: with no BlueRoute lease it is a no-op and cannot disable foreign forwarding.
    backend.set_ipv4_forwarding(false).await?;
    backend.set_ipv4_forwarding(false).await?;
    let baseline = read_forwarding()?;
    println!("baseline IPv4 forwarding: {}", bit(baseline));

    if Path::new(LEASE_PATH).exists() {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "BlueRoute forwarding lease remains after repeated startup cleanup",
        )
        .into());
    }

    backend.set_ipv4_forwarding(true).await?;
    backend.set_ipv4_forwarding(true).await?;
    if !read_forwarding()? {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "IPv4 forwarding is not enabled after repeated BlueRoute enable",
        )
        .into());
    }
    let lease = fs::read_to_string(LEASE_PATH)?;
    println!(
        "forwarding enabled: kernel=1 lease={}",
        lease.trim().replace('\n', ";")
    );

    // Reconnect through a fresh backend object so success cannot depend on process-local state.
    let reconnected = NetworkManagerBackend::connect_system().await?;
    reconnected.set_ipv4_forwarding(true).await?;
    if !read_forwarding()? {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "fresh backend connection did not preserve enabled IPv4 forwarding",
        )
        .into());
    }
    println!("forwarding lease rediscovered by fresh backend connection");

    println!(
        "holding IPv4 forwarding for {hold_seconds}s; independently inspect {IPV4_FORWARD_SYSCTL} and {LEASE_PATH}"
    );
    Timer::after(Duration::from_secs(hold_seconds)).await;

    reconnected.set_ipv4_forwarding(false).await?;
    reconnected.set_ipv4_forwarding(false).await?;
    let restored = read_forwarding()?;
    if restored != baseline {
        return Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "IPv4 forwarding did not return to the pre-probe baseline",
            format!(
                "baseline={} restored={}",
                bit(baseline),
                bit(restored)
            ),
        )
        .into());
    }
    if Path::new(LEASE_PATH).exists() {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "BlueRoute forwarding lease remains after repeated release",
        )
        .into());
    }

    println!(
        "forwarding restored: baseline={} repeated-release=succeeded lease=absent",
        bit(baseline)
    );
    println!("P4-009 IPv4 forwarding probe PASS");
    Ok(())
}

fn read_forwarding() -> Result<bool, CoreError> {
    let value = fs::read_to_string(IPV4_FORWARD_SYSCTL).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::NetworkBackendUnavailable,
            "failed to inspect kernel IPv4 forwarding state",
            format!("path={IPV4_FORWARD_SYSCTL} error={error}"),
        )
    })?;
    match value.trim() {
        "0" => Ok(false),
        "1" => Ok(true),
        other => Err(CoreError::with_diagnostic(
            ErrorKind::InvalidState,
            "kernel IPv4 forwarding state is not 0 or 1",
            format!("path={IPV4_FORWARD_SYSCTL} value={other:?}"),
        )),
    }
}

fn bit(value: bool) -> u8 {
    u8::from(value)
}

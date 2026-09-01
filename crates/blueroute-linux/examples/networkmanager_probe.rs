use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_core::{CoreError, ErrorKind, IpPrefix, NetworkId};
use blueroute_linux::{
    InterfaceAddress, IpNetworkBackend, NetworkConnection, NetworkDeviceHandle,
    NetworkInterfaceHandle, NetworkManagerBackend, NetworkStateBackend, NetworkStateEvent,
    NetworkStateSubscription,
};
use futures_lite::future::race;

const PROBE_OWNER: NetworkId = NetworkId::from_bytes([0x47; 16]);
const OTHER_OWNER: NetworkId = NetworkId::from_bytes([0x48; 16]);
const PROBE_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 254, 89, 1));
const PROBE_PREFIX_LEN: u8 = 30;
const EVENT_TIMEOUT: Duration = Duration::from_secs(8);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let bridge_name = env::args()
        .nth(1)
        .unwrap_or_else(|| "br-blue-nm".to_owned());
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

    let backend = NetworkManagerBackend::connect_system().await?;
    println!("NetworkManager version: {}", backend.version().await?);

    // A previous interrupted probe may have left only this probe owner's state behind.
    // Removing it is safe because cleanup is owner-scoped and idempotent.
    backend
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;
    backend
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;

    let baseline_connections = backend.network_connections().await?;
    let baseline_devices = backend.network_devices().await?;
    let foreign_profiles: Vec<NetworkConnection> = baseline_connections
        .iter()
        .filter(|profile| profile.owner.is_none())
        .cloned()
        .collect();
    println!(
        "baseline: connections={} devices={} foreign-profiles={}",
        baseline_connections.len(),
        baseline_devices.len(),
        foreign_profiles.len()
    );

    let mut subscription = backend.subscribe_network_state().await?;

    let first = backend.ensure_bridge(PROBE_OWNER, bridge.clone()).await?;
    let second = backend.ensure_bridge(PROBE_OWNER, bridge.clone()).await?;
    if first.handle != second.handle {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "repeated ensure_bridge created a different NetworkManager profile",
        )
        .into());
    }
    if first.owner != Some(PROBE_OWNER) || first.interface.as_ref() != Some(&bridge) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "NetworkManager bridge profile does not carry the expected BlueRoute ownership",
        )
        .into());
    }
    println!(
        "bridge ready: interface={} profile={} owner={} repeated-ensure=same-profile",
        bridge.as_str(),
        first.handle.as_str(),
        PROBE_OWNER
    );

    let bridge_device = observe_bridge_present(&mut *subscription, &bridge, PROBE_OWNER).await?;

    backend.ensure_address(address.clone()).await?;
    backend.ensure_address(address.clone()).await?;
    if !backend.addresses().await?.contains(&address) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "BlueRoute-owned address is missing after repeated ensure_address",
        )
        .into());
    }
    println!(
        "address ready: interface={} address={}/{} repeated-ensure=present",
        bridge.as_str(),
        PROBE_ADDRESS,
        PROBE_PREFIX_LEN
    );

    assert_foreign_profiles_preserved(&foreign_profiles, &backend.network_connections().await?)?;
    println!("foreign NetworkManager profiles preserved after setup");

    match backend.ensure_bridge(OTHER_OWNER, bridge.clone()).await {
        Err(error) if error.kind() == ErrorKind::InvalidState => {
            println!("cross-owner bridge takeover rejected: {error}");
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "a second BlueRoute owner unexpectedly claimed the active bridge",
            )
            .into());
        }
    }
    let after_takeover_probe = backend.network_connections().await?;
    if after_takeover_probe.iter().any(|profile| {
        profile.owner == Some(OTHER_OWNER) && profile.interface.as_ref() == Some(&bridge)
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "rejected cross-owner bridge takeover leaked a NetworkManager profile",
        )
        .into());
    }
    println!("cross-owner rejection left no leaked profile");

    backend
        .remove_owned_interface(OTHER_OWNER, bridge.clone())
        .await?;
    if !backend
        .network_connections()
        .await?
        .iter()
        .any(|profile| profile.handle == first.handle && profile.owner == Some(PROBE_OWNER))
    {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "wrong-owner cleanup removed the probe owner's NetworkManager profile",
        )
        .into());
    }
    println!("wrong-owner cleanup was a safe no-op");

    println!(
        "holding configured bridge for {hold_seconds}s; verify the live kernel address in another terminal with: ip -4 addr show {}",
        bridge.as_str()
    );
    Timer::after(Duration::from_secs(hold_seconds)).await;

    backend.remove_address(address.clone()).await?;
    backend.remove_address(address.clone()).await?;
    if backend.addresses().await?.contains(&address) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "BlueRoute-owned address remains after repeated remove_address",
        )
        .into());
    }
    println!("address removed; repeated remove succeeded");

    backend
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;
    backend
        .remove_owned_interface(PROBE_OWNER, bridge.clone())
        .await?;
    observe_bridge_absent(&mut *subscription, &bridge, &first, &bridge_device).await?;

    let final_connections = backend.network_connections().await?;
    if final_connections.iter().any(|profile| {
        profile.owner == Some(PROBE_OWNER) && profile.interface.as_ref() == Some(&bridge)
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "BlueRoute-owned NetworkManager profile remains after cleanup",
        )
        .into());
    }
    assert_foreign_profiles_preserved(&foreign_profiles, &final_connections)?;
    println!("bridge/profile removed; repeated cleanup succeeded");
    println!("foreign NetworkManager profiles preserved after cleanup");
    println!("P4-007 NetworkManager probe PASS");
    Ok(())
}

async fn observe_bridge_present(
    subscription: &mut dyn NetworkStateSubscription,
    bridge: &NetworkInterfaceHandle,
    owner: NetworkId,
) -> Result<NetworkDeviceHandle, CoreError> {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    let mut connection_seen = false;
    let mut device_handle = None;
    while !connection_seen || device_handle.is_none() {
        let event = next_event_before(subscription, deadline).await?;
        match event {
            NetworkStateEvent::ConnectionAdded(profile)
            | NetworkStateEvent::ConnectionChanged(profile)
                if profile.owner == Some(owner) && profile.interface.as_ref() == Some(bridge) =>
            {
                connection_seen = true;
                println!(
                    "observed NetworkManager connection event for {}",
                    bridge.as_str()
                );
            }
            NetworkStateEvent::DeviceAdded(device) | NetworkStateEvent::DeviceChanged(device)
                if &device.interface == bridge =>
            {
                println!(
                    "observed NetworkManager device event for {}",
                    bridge.as_str()
                );
                device_handle = Some(device.handle);
            }
            _ => {}
        }
    }
    device_handle.ok_or_else(|| {
        CoreError::new(
            ErrorKind::Internal,
            "NetworkManager bridge event loop completed without a device handle",
        )
    })
}

async fn observe_bridge_absent(
    subscription: &mut dyn NetworkStateSubscription,
    bridge: &NetworkInterfaceHandle,
    profile: &NetworkConnection,
    device: &NetworkDeviceHandle,
) -> Result<(), CoreError> {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    let mut connection_removed = false;
    let mut device_removed = false;
    while !(connection_removed && device_removed) {
        let event = next_event_before(subscription, deadline).await?;
        match event {
            NetworkStateEvent::ConnectionRemoved(handle) if handle == profile.handle => {
                connection_removed = true;
                println!(
                    "observed NetworkManager connection removal for {}",
                    bridge.as_str()
                );
            }
            NetworkStateEvent::DeviceRemoved(handle) if &handle == device => {
                device_removed = true;
                println!(
                    "observed NetworkManager device removal for {}",
                    bridge.as_str()
                );
            }
            _ => {}
        }
    }
    Ok(())
}

async fn next_event_before(
    subscription: &mut dyn NetworkStateSubscription,
    deadline: Instant,
) -> Result<NetworkStateEvent, CoreError> {
    let now = Instant::now();
    if now >= deadline {
        return Err(CoreError::new(
            ErrorKind::NetworkBackendUnavailable,
            "timed out waiting for a NetworkManager state event",
        ));
    }
    enum Completion {
        Event(Option<NetworkStateEvent>),
        Timeout,
    }
    let completion = race(
        async { subscription.next_event().await.map(Completion::Event) },
        async {
            Timer::after(deadline.saturating_duration_since(now)).await;
            Ok(Completion::Timeout)
        },
    )
    .await?;
    match completion {
        Completion::Event(Some(event)) => Ok(event),
        Completion::Event(None) => Err(CoreError::new(
            ErrorKind::NetworkBackendUnavailable,
            "NetworkManager state subscription ended unexpectedly",
        )),
        Completion::Timeout => Err(CoreError::new(
            ErrorKind::NetworkBackendUnavailable,
            "timed out waiting for a NetworkManager state event",
        )),
    }
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
                "NetworkManager profile present before the probe disappeared",
                format!("profile={} id={}", original.handle.as_str(), original.id),
            ));
        };
        if current.id != original.id || current.uuid != original.uuid || current.owner.is_some() {
            return Err(CoreError::with_diagnostic(
                ErrorKind::InvalidState,
                "foreign NetworkManager profile changed ownership or identity during the probe",
                format!("profile={} id={}", original.handle.as_str(), original.id),
            ));
        }
    }
    Ok(())
}

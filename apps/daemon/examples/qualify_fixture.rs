//! TEST-ONLY fixture for CR-QUALIFY Part A (FU-06). Never packaged: the Debian build compiles
//! only the daemon, CLI and TUI binaries, and examples are not installed.
//!
//! On a disposable VM, as root, it creates BlueRoute-owned state with the production stores and
//! the production NetworkManager adapter, so the metadata is exactly what the daemon writes, and
//! no Bluetooth adapter is needed:
//!
//! ```text
//! qualify_fixture host    <state-dir>             # known hosted network: owned bridge + address
//! qualify_fixture client  <state-dir> <iface>     # known joined client: owned address on <iface>
//!                                                 #   plus a client record (Bluetooth host hint)
//! qualify_fixture foreign <state-dir>             # owned bridge + address of a network the
//!                                                 #   registry does NOT know (another install)
//! ```
//!
//! `<iface>` must already exist and be known to NetworkManager (for example a dummy interface
//! created for the test); it stands in for a PANU `bnep` interface. Each run prints the
//! NetworkId it created. It grants nothing: the removal helper decides ownership exactly as for
//! real state, and this tool is not on any production path.
use std::error::Error;
use std::path::PathBuf;

use blueroute_core::{
    DaemonConfig, DisplayName, Ipv4StarAddressPlan, MembershipRole, MembershipState,
    NetworkMembership, NodeId,
};
use blueroute_daemon::{
    ClientRecord, ClientRecordStore, NetworkIdGenerator, SystemNetworkIdGenerator,
};
use blueroute_linux::{
    InterfaceAddress, IpNetworkBackend, NetworkInterfaceHandle, NetworkManagerBackend,
    NetworkMembershipStore, NetworkStateBackend, PeerHandle,
};
use futures_lite::future;

/// The hint a joined client records for its host's Bluetooth device. Fake: no such device.
const FIXTURE_HOST_DEVICE: &str = "/org/bluez/hci0/dev_02_00_00_00_00_01";

fn main() {
    if let Err(error) = future::block_on(run()) {
        eprintln!("qualify_fixture failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (mode, state, interface) = match arguments.as_slice() {
        [mode, state] if mode == "host" || mode == "foreign" => (mode.as_str(), state, None),
        [mode, state, interface] if mode == "client" => (mode.as_str(), state, Some(interface)),
        _ => {
            eprintln!(
                "usage: qualify_fixture host|foreign <state-dir> | client <state-dir> <iface>"
            );
            std::process::exit(2);
        }
    };
    let state = PathBuf::from(state);
    let network = SystemNetworkIdGenerator.generate()?;
    let plan =
        Ipv4StarAddressPlan::for_network(network, DaemonConfig::default().ipv4_address_pool)?;
    let backend = NetworkManagerBackend::connect_system().await?;

    if mode == "client" {
        let interface = NetworkInterfaceHandle::new(interface.expect("checked above").as_str())?;
        backend
            .ensure_address(InterfaceAddress {
                interface,
                prefix: plan.first_client,
                owner: network,
            })
            .await?;
    } else {
        let id = network.to_string();
        let bridge = NetworkInterfaceHandle::new(format!("brb-{}", &id[..8]))?;
        backend.ensure_bridge(network, bridge.clone()).await?;
        backend
            .ensure_address(InterfaceAddress {
                interface: bridge,
                prefix: plan.host,
                owner: network,
            })
            .await?;
    }

    if mode != "foreign" {
        let store = NetworkMembershipStore::new(state.join("memberships-v1"));
        let mut registry = store.load()?;
        let mut membership = NetworkMembership::new(network, DisplayName::new("QA fixture")?);
        membership.state = MembershipState::Member;
        membership.role = Some(if mode == "client" {
            MembershipRole::Client
        } else {
            MembershipRole::Host
        });
        registry.remember_network(membership);
        store.save(&registry)?;
    }
    if mode == "client" {
        let record = ClientRecord::new(
            network,
            NodeId::from_bytes([0x51; 16]),
            Some(PeerHandle::new(FIXTURE_HOST_DEVICE)?),
        )?;
        ClientRecordStore::new(state.join("client-record")).save(Some(&record))?;
    }
    println!("{network}");
    Ok(())
}

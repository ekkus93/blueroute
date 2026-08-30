use std::env;
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

use blueroute_linux::{BluetoothBackend, BluezBackend, DiscoveredPeer};
use futures_lite::future;

const DISCOVERY_WINDOW: Duration = Duration::from_secs(10);

fn main() -> Result<(), Box<dyn Error>> {
    let target = env::args().nth(1).ok_or_else(|| {
        io::Error::other("usage: bluez_pair_probe <exact display name or BlueZ peer path>")
    })?;

    future::block_on(async move {
        let backend = BluezBackend::connect_system().await?;
        let adapter = backend
            .adapters()
            .await?
            .into_iter()
            .find(|adapter| adapter.powered)
            .ok_or_else(|| io::Error::other("no powered Bluetooth adapter is available"))?;

        println!("adapter: {}", adapter.handle.as_str());
        println!("discovering for {} seconds...", DISCOVERY_WINDOW.as_secs());
        backend.start_discovery(adapter.handle.clone()).await?;
        thread::sleep(DISCOVERY_WINDOW);
        let peers = backend.discovered_peers(adapter.handle.clone()).await;
        let stop = backend.stop_discovery(adapter.handle.clone()).await;
        let peers = peers?;
        stop?;

        let peer = find_peer(&peers, &target).ok_or_else(|| {
            io::Error::other(format!("Bluetooth peer {target:?} was not discovered"))
        })?;
        println!(
            "target: {} name={} paired={} trusted={}",
            peer.handle.as_str(),
            peer.display_name.as_deref().unwrap_or("<unknown>"),
            peer.paired,
            peer.trusted
        );

        backend.pair(peer.handle.clone()).await?;
        backend.set_trusted(peer.handle.clone(), true).await?;

        let refreshed = backend
            .discovered_peers(adapter.handle)
            .await?
            .into_iter()
            .find(|candidate| candidate.handle == peer.handle)
            .ok_or_else(|| io::Error::other("paired peer disappeared from BlueZ"))?;
        if !refreshed.paired || !refreshed.trusted {
            return Err(io::Error::other(format!(
                "pairing did not converge: paired={} trusted={}",
                refreshed.paired, refreshed.trusted
            ))
            .into());
        }
        println!("pairing complete: paired=true trusted=true");
        Ok::<(), Box<dyn Error>>(())
    })
}

fn find_peer<'a>(peers: &'a [DiscoveredPeer], target: &str) -> Option<&'a DiscoveredPeer> {
    peers
        .iter()
        .find(|peer| peer.handle.as_str() == target || peer.display_name.as_deref() == Some(target))
}

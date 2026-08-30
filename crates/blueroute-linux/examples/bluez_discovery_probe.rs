use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

use blueroute_linux::{BluezBackend, BluetoothBackend};
use futures_lite::future;

const DISCOVERY_WINDOW: Duration = Duration::from_secs(10);

fn main() -> Result<(), Box<dyn Error>> {
    future::block_on(async {
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
        let stop_result = backend.stop_discovery(adapter.handle).await;
        let peers = peers?;
        stop_result?;

        println!("discovered {} BlueZ device object(s)", peers.len());
        for peer in peers {
            println!(
                "{}\tname={}\tpaired={}\ttrusted={}",
                peer.handle.as_str(),
                peer.display_name.as_deref().unwrap_or("<unknown>"),
                peer.paired,
                peer.trusted
            );
        }
        Ok::<(), Box<dyn Error>>(())
    })
}

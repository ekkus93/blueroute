use std::error::Error;
use std::time::Duration;

use async_io::Timer;
use blueroute_linux::{BluetoothBackend, BluezBackend};
use futures_lite::future;

const PAIRING_WINDOW: Duration = Duration::from_secs(120);

fn main() -> Result<(), Box<dyn Error>> {
    future::block_on(async move {
        let backend = BluezBackend::connect_system().await?;
        let adapter = backend
            .adapters()
            .await?
            .into_iter()
            .find(|adapter| adapter.powered)
            .ok_or("no powered Bluetooth adapter is available")?;

        let window = backend
            .begin_incoming_pairing(adapter.handle.clone())
            .await?;
        println!("adapter: {}", window.adapter().as_str());
        println!(
            "Rust-controlled incoming pairing window open for {} seconds...",
            PAIRING_WINDOW.as_secs()
        );
        println!("Run bluez_pair_probe from the other Linux test node now.");
        Timer::after(PAIRING_WINDOW).await;
        backend.end_incoming_pairing(window).await?;

        let paired = backend
            .discovered_peers(adapter.handle)
            .await?
            .into_iter()
            .filter(|peer| peer.paired)
            .collect::<Vec<_>>();
        println!(
            "pairing window closed; {} paired peer(s) visible",
            paired.len()
        );
        for peer in paired {
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

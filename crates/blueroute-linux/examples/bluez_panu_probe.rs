use std::env;
use std::time::Duration;

use async_io::Timer;
use blueroute_linux::{BluetoothBackend, BluezBackend, PanBackend, PanuEvent};
use futures_lite::future::race;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let target_name = env::args().nth(1).ok_or(
        "usage: cargo run -p blueroute-linux --example bluez_panu_probe --locked -- <peer-name> [hold-seconds]",
    )?;
    let hold_seconds = env::args()
        .nth(2)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(60);

    let backend = BluezBackend::connect_system().await?;
    let adapter = backend
        .adapters()
        .await?
        .into_iter()
        .find(|adapter| adapter.powered)
        .ok_or("no powered Bluetooth adapter is available")?;

    println!("adapter: {}", adapter.handle.as_str());
    println!("discovering for 10 seconds...");
    backend.start_discovery(adapter.handle.clone()).await?;
    Timer::after(Duration::from_secs(10)).await;
    let peers = backend.discovered_peers(adapter.handle.clone()).await;
    let stop_result = backend.stop_discovery(adapter.handle).await;
    let peers = peers?;
    stop_result?;

    let peer = peers
        .into_iter()
        .find(|peer| peer.display_name.as_deref() == Some(target_name.as_str()))
        .ok_or_else(|| format!("Bluetooth peer {target_name:?} was not discovered"))?;

    println!(
        "target: {} name={} paired={} trusted={}",
        peer.handle.as_str(),
        peer.display_name.as_deref().unwrap_or("<unnamed>"),
        peer.paired,
        peer.trusted
    );

    let attachment = backend.connect_panu(peer.handle.clone()).await?;
    println!(
        "PANU connected: interface={} hold={}s",
        attachment.interface.as_str(),
        hold_seconds
    );
    println!(
        "The BNEP link is up. IP addressing is intentionally outside P4-005; configure test addresses separately if validating the data plane."
    );

    let mut subscription = backend.subscribe_panu_events(attachment.clone()).await?;
    enum Completion {
        Lost(Option<PanuEvent>),
        HoldElapsed,
    }

    let completion = race(
        async { subscription.next_event().await.map(Completion::Lost) },
        async {
            Timer::after(Duration::from_secs(hold_seconds)).await;
            Ok(Completion::HoldElapsed)
        },
    )
    .await?;

    match completion {
        Completion::Lost(Some(PanuEvent::Lost(lost))) => {
            println!(
                "PANU link lost: peer={} interface={}",
                lost.peer
                    .as_ref()
                    .map(|peer| peer.as_str())
                    .unwrap_or("<unknown>"),
                lost.interface.as_str()
            );
        }
        Completion::Lost(None) => println!("PANU event stream ended"),
        Completion::HoldElapsed => println!("PANU hold window elapsed"),
    }

    backend.disconnect_panu(peer.handle.clone()).await?;
    backend.disconnect_panu(peer.handle).await?;
    println!("PANU disconnected; repeated disconnect succeeded");
    Ok(())
}

use std::env;
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_linux::{
    BluetoothBackend, BluezBackend, NapEvent, NetworkInterfaceHandle, PanBackend,
};
use futures_lite::future::race;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let bridge_name = env::args().nth(1).ok_or(
        "usage: cargo run -p blueroute-linux --example bluez_nap_probe --locked -- <bridge-interface> [hold-seconds]",
    )?;
    let hold_seconds = env::args()
        .nth(2)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(120);
    let bridge = NetworkInterfaceHandle::new(bridge_name)?;

    let backend = BluezBackend::connect_system().await?;
    let adapter = backend
        .adapters()
        .await?
        .into_iter()
        .find(|adapter| adapter.powered)
        .ok_or("no powered Bluetooth adapter is available")?;

    println!("adapter: {}", adapter.handle.as_str());
    let attachment = backend.start_nap(adapter.handle.clone(), bridge).await?;
    println!(
        "NAP registered: bridge={} hold={}s",
        attachment.interface.as_str(),
        hold_seconds
    );
    println!(
        "BlueZ now accepts PANU clients into the supplied bridge. Bridge creation and IP addressing are intentionally outside P4-006."
    );

    let observation = observe_clients(
        &backend,
        adapter.handle.clone(),
        attachment,
        Duration::from_secs(hold_seconds),
    )
    .await;

    // Always attempt teardown even if observation failed, and call it twice to exercise
    // desired-state idempotence on real BlueZ hardware.
    let first_stop = backend.stop_nap(adapter.handle.clone()).await;
    let second_stop = backend.stop_nap(adapter.handle).await;

    observation?;
    first_stop?;
    second_stop?;
    println!("NAP stopped; repeated stop succeeded");
    Ok(())
}

async fn observe_clients(
    backend: &BluezBackend,
    adapter: blueroute_linux::AdapterHandle,
    attachment: blueroute_linux::PanAttachment,
    hold: Duration,
) -> Result<(), blueroute_core::CoreError> {
    let mut subscription = backend.subscribe_nap_events(adapter, attachment).await?;
    let deadline = Instant::now() + hold;

    loop {
        let now = Instant::now();
        if now >= deadline {
            println!("NAP hold window elapsed");
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(now);

        enum Completion {
            Event(Option<NapEvent>),
            HoldElapsed,
        }

        let completion = race(
            async { subscription.next_event().await.map(Completion::Event) },
            async {
                Timer::after(remaining).await;
                Ok(Completion::HoldElapsed)
            },
        )
        .await?;

        match completion {
            Completion::Event(Some(NapEvent::ClientAttached(client))) => println!(
                "NAP client attached: interface={}",
                client.interface.as_str()
            ),
            Completion::Event(Some(NapEvent::ClientDetached(client))) => println!(
                "NAP client detached: interface={}",
                client.interface.as_str()
            ),
            Completion::Event(None) => {
                println!("NAP event stream ended");
                return Ok(());
            }
            Completion::HoldElapsed => {
                println!("NAP hold window elapsed");
                return Ok(());
            }
        }
    }
}

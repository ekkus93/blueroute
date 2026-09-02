use std::error::Error;
use std::time::Duration;

use async_io::Timer;
use blueroute_client::BlueRouteClient;
use blueroute_protocol::{Command, Response};
use futures_lite::future;

const DISCOVERY_WINDOW: Duration = Duration::from_secs(10);

fn main() {
    if let Err(error) = future::block_on(run()) {
        eprintln!("BlueRoute network discovery probe failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let client = BlueRouteClient::connect().await?;
    expect_ack(client.request(&Command::StartDiscovery).await?)?;
    println!(
        "discovering BlueRoute networks for {} seconds...",
        DISCOVERY_WINDOW.as_secs()
    );

    Timer::after(DISCOVERY_WINDOW).await;
    let discovered = client.request(&Command::ListNetworks).await;
    let stop = client.request(&Command::StopDiscovery).await;

    let response = discovered?;
    expect_ack(stop?)?;
    let Response::Networks(networks) = response else {
        return Err(std::io::Error::other(
            "daemon returned a non-network response to ListNetworks",
        )
        .into());
    };

    if networks.is_empty() {
        println!("no BlueRoute networks discovered");
    } else {
        for network in networks {
            println!(
                "network={} name={} member_count={}",
                network.id, network.name, network.member_count
            );
        }
    }
    Ok(())
}

fn expect_ack(response: Response) -> Result<(), Box<dyn Error>> {
    if response == Response::Ack {
        Ok(())
    } else {
        Err(std::io::Error::other("daemon returned a non-ack response").into())
    }
}

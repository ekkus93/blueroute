use blueroute_linux::SystemCapabilityProbe;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let report = SystemCapabilityProbe::default().report().await?;

    println!("support: {:?}", report.support);
    println!(
        "BlueZ: available={} version={}",
        report.runtime.bluez_available,
        report.bluez_version.as_deref().unwrap_or("unknown")
    );
    println!(
        "network backend: {:?} version={}",
        report.runtime.network_backend,
        report.network_backend_version.as_deref().unwrap_or("unknown")
    );
    println!(
        "kernel: {}",
        report.kernel_release.as_deref().unwrap_or("unknown")
    );
    println!("controllers: {}", report.controllers.len());
    for controller in &report.controllers {
        println!(
            "  {} powered={} address={} driver={}",
            controller.handle,
            controller.powered,
            controller.address.as_deref().unwrap_or("unknown"),
            controller.driver.as_deref().unwrap_or("unknown")
        );
    }
    println!("PANU available: {}", report.panu_available);
    println!(
        "NAP available: {}",
        report
            .nap_available
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    println!(
        "IPv4 forwarding: available={} enabled={}",
        report.forwarding_available,
        report
            .forwarding_enabled
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    println!(
        "peer ceiling: practical={} configured={} effective={}",
        report.practical_peer_ceiling,
        report
            .configured_peer_ceiling
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".into()),
        report.effective_peer_ceiling
    );
    println!("runtime prerequisites:");
    for prerequisite in &report.prerequisites {
        println!(
            "  {} available={} detail={}",
            prerequisite.name, prerequisite.available, prerequisite.detail
        );
    }
    println!("diagnostics:");
    for diagnostic in &report.diagnostics {
        println!(
            "  {:?} [{}] {}",
            diagnostic.level, diagnostic.component, diagnostic.message
        );
    }

    Ok(())
}

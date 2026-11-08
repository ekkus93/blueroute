//! Tauri 2 shell for the BlueRoute desktop UI.
//!
//! The frontend can call only the six commands below: four read-only snapshots plus
//! `create_network` and `leave_network`. Each connects through
//! the shared version-gated `blueroute-client` (via `blueroute-desktop`), so the web
//! content never gets D-Bus, shell, filesystem or arbitrary daemon-command access, and
//! every failure reaches it as a coarse, typed `DesktopError`. Daemon changes reach it only
//! as payload-free `daemon-signal` events (connected / changed / disconnected).
use blueroute_desktop::{
    DesktopBridge, DesktopError, DesktopOverview, PlatformCheck, watch_daemon,
};
use blueroute_protocol::{NetworkSummary, NodeSummary};
use tauri::Emitter;

#[tauri::command]
async fn platform_check() -> Result<PlatformCheck, DesktopError> {
    DesktopBridge::connect().await?.platform_check().await
}

#[tauri::command]
async fn overview() -> Result<DesktopOverview, DesktopError> {
    DesktopBridge::connect().await?.overview().await
}

#[tauri::command]
async fn networks() -> Result<Vec<NetworkSummary>, DesktopError> {
    DesktopBridge::connect().await?.networks().await
}

/// A mutating command. The daemon authorizes it through PolicyKit.
#[tauri::command]
async fn create_network(name: String) -> Result<(), DesktopError> {
    DesktopBridge::connect().await?.create_network(&name).await
}

/// The other mutating command; takes no arguments. The daemon authorizes it through PolicyKit.
#[tauri::command]
async fn leave_network() -> Result<(), DesktopError> {
    DesktopBridge::connect().await?.leave_network().await
}

#[tauri::command]
async fn devices() -> Result<Vec<NodeSummary>, DesktopError> {
    DesktopBridge::connect().await?.devices().await
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            platform_check,
            overview,
            networks,
            devices,
            create_network,
            leave_network
        ])
        .setup(|app| {
            // Payload-free signals only; the UI re-reads typed snapshots on each one.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(watch_daemon(move |signal| {
                let _ = handle.emit("daemon-signal", signal);
            }));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run the BlueRoute desktop application");
}

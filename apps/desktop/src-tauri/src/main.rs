//! Tauri 2 shell for the BlueRoute desktop UI.
//!
//! The frontend can call only the nine commands below: five read-only snapshots plus
//! `create_network`, `leave_network`, `retry_client_reconnect` and `set_device_name`. Each
//! connects through
//! the shared version-gated `blueroute-client` (via `blueroute-desktop`), so the web
//! content never gets D-Bus, shell, filesystem or arbitrary daemon-command access, and
//! every failure reaches it as a coarse, typed `DesktopError`. Daemon changes reach it only
//! as payload-free `daemon-signal` events (connected / changed / disconnected).
use blueroute_desktop::{
    DesktopBridge, DesktopError, DesktopOverview, PlatformCheck, watch_daemon,
};
use blueroute_protocol::{ClientLinkState, NetworkSummary, NodeSummary};
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

/// A mutating command with no arguments. The daemon authorizes it through PolicyKit.
#[tauri::command]
async fn leave_network() -> Result<(), DesktopError> {
    DesktopBridge::connect().await?.leave_network().await
}

/// A mutating command with no arguments: ask a joined client to try reaching its host again.
/// The daemon authorizes it through PolicyKit.
#[tauri::command]
async fn retry_client_reconnect() -> Result<(), DesktopError> {
    DesktopBridge::connect()
        .await?
        .retry_client_reconnect()
        .await
}

/// A mutating command: rename this computer (a label, never its identity). The daemon
/// authorizes it through PolicyKit.
#[tauri::command]
async fn set_device_name(name: String) -> Result<(), DesktopError> {
    DesktopBridge::connect().await?.set_device_name(&name).await
}

#[tauri::command]
async fn client_link() -> Result<Option<ClientLinkState>, DesktopError> {
    DesktopBridge::connect().await?.client_link().await
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
            client_link,
            create_network,
            leave_network,
            retry_client_reconnect,
            set_device_name
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

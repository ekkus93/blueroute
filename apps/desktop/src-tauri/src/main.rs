//! Tauri 2 shell for the BlueRoute desktop UI.
//!
//! The frontend can call only the four read-only commands below. Each connects through
//! the shared version-gated `blueroute-client` (via `blueroute-desktop`), so the web
//! content never gets D-Bus, shell, filesystem or arbitrary daemon-command access, and
//! every failure reaches it as a coarse, typed `DesktopError`.
use blueroute_desktop::{DesktopBridge, DesktopError, DesktopOverview, PlatformCheck};
use blueroute_protocol::{NetworkSummary, NodeSummary};

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
            devices
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the BlueRoute desktop application");
}

//! Tauri 2 shell for the BlueRoute desktop UI (P12-001 bootstrap).
//!
//! This binary intentionally registers no commands and no plugins. Daemon access
//! arrives later through narrow typed commands backed by `blueroute-desktop`
//! (P12-002); the frontend never gets direct D-Bus, shell or filesystem access.

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run the BlueRoute desktop application");
}

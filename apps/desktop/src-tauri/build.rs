//! Declares the application's complete Tauri command surface. Every command must be
//! listed here and individually granted in `capabilities/`; nothing is allowed by default.
const COMMANDS: &[&str] = &["platform_check", "overview", "networks", "devices"];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}

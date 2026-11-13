//! Architecture guard (P10-001 acceptance, CLAUDE.md "Architecture rules"): front ends talk to the
//! daemon only through `blueroute-client`. None may depend on the Linux adapter crate, name the
//! BlueZ or NetworkManager D-Bus services, or start processes (production code never shells out).

use std::path::{Path, PathBuf};

const FRONT_ENDS: [&str; 4] = ["cli", "tui", "desktop", "desktop/src-tauri"];

fn apps_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("apps directory")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn front_ends_never_reach_bluetooth_or_networking_directly() {
    let forbidden_dependencies = ["blueroute-linux"];
    let forbidden_source = [
        "blueroute_linux",
        "org.bluez",
        "org.freedesktop.NetworkManager",
        // Spawning a process; `std::process::exit` is fine.
        "process::Command",
        "Command::new(",
        "Stdio::",
    ];
    let mut checked = 0;
    for front_end in FRONT_ENDS {
        let root = apps_dir().join(front_end);
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("manifest");
        for name in forbidden_dependencies {
            assert!(
                !manifest.contains(name),
                "{front_end}/Cargo.toml depends on {name}"
            );
        }
        let mut sources = Vec::new();
        rust_sources(&root.join("src"), &mut sources);
        assert!(!sources.is_empty(), "{front_end} has no Rust sources");
        for path in sources {
            let text = std::fs::read_to_string(&path).expect("source");
            for needle in forbidden_source {
                assert!(
                    !text.contains(needle),
                    "{} contains {needle:?}",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= FRONT_ENDS.len());
}

#[test]
fn the_cli_links_dbus_only_through_the_shared_client() {
    let manifest = include_str!("../Cargo.toml");
    let (dependencies, dev) = manifest
        .split_once("[dev-dependencies]")
        .expect("the CLI has dev-dependencies");
    assert!(dependencies.contains("blueroute-client"));
    assert!(
        !dependencies.contains("zbus"),
        "zbus must stay a test-only dependency"
    );
    assert!(dev.contains("zbus"));
}

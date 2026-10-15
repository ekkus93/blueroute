//! Executable CLI exit-contract tests; no production daemon or Bluetooth
//! hardware is required. All errors here are intentionally local/synthetic.

use std::process::{Command, Output};

fn invoke(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blueroute"))
        .args(arguments)
        .output()
        .expect("CLI binary should run in test environment")
}

#[test]
fn help_and_version_do_not_require_daemon_and_exit_zero() {
    for args in [&["--help"][..], &["--version"][..]] {
        let result = invoke(args);
        assert_eq!(result.status.code(), Some(0), "args={args:?}");
        assert!(result.stderr.is_empty());
    }
}

#[test]
fn invalid_and_duplicate_options_exit_usage_without_contacting_daemon() {
    for args in [
        &["--unrecognized"][..],
        &["--json", "--json", "status"][..],
        &["diagnostics"][..],
        &["network", "join"][..],
        &["diagnostics", "show", "unexpected"][..],
    ] {
        let result = invoke(args);
        assert_eq!(result.status.code(), Some(2), "args={args:?}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("Usage: blueroute"),
            "args={args:?}: {stderr}"
        );
    }
}

#[test]
fn daemon_unavailable_exits_three_without_falling_back_to_privileged_tools() {
    // /dev/null is not a D-Bus socket. Even on a developer machine with a
    // functioning real system bus this forces local connection failure.
    let result = Command::new(env!("CARGO_BIN_EXE_blueroute"))
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/dev/null")
        .args(["diagnostics", "show", "--json"])
        .output()
        .expect("CLI binary should run in test environment");

    assert_eq!(result.status.code(), Some(3));
    assert!(result.stdout.is_empty());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("daemon unavailable/incompatible"),
        "unexpected connection diagnostic: {error}"
    );
}

#[test]
#[ignore = "requires dbus-run-session; CI starts an isolated broker"]
fn cli_exit_statuses_propagate_typed_broker_failures() -> Result<(), Box<dyn std::error::Error>> {
    use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME};
    use zbus::{connection::Builder, fdo, interface};

    struct FakeDaemon;

    #[interface(name = "org.blueroute.Service1")]
    impl FakeDaemon {
        fn version(&self) -> (u16, u16) {
            (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor)
        }

        fn request(&self, payload: &str) -> fdo::Result<String> {
            let json: serde_json::Value = serde_json::from_str(payload)
                .map_err(|_| fdo::Error::InvalidArgs("malformed fixture request".into()))?;
            match json["type"].as_str() {
                Some("get_diagnostics") => {
                    Err(fdo::Error::NotSupported("mock platform capability unavailable".into()))
                }
                Some("trust_peer") => {
                    Err(fdo::Error::AccessDenied("mock PolicyKit authorization denied".into()))
                }
                Some("list_networks") => {
                    // Spoofed diagnostic TEXT must not be treated as a typed AccessDenied.
                    Err(fdo::Error::Failed("AccessDenied string is not an auth error".into()))
                }
                _ => Err(fdo::Error::InvalidArgs("unexpected fixture command".into())),
            }
        }
    }

    futures_lite::future::block_on(async {
        let _server = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, FakeDaemon)?
            .build()
            .await?;
        let broker_address = std::env::var("DBUS_SESSION_BUS_ADDRESS")?;
        let peer = blueroute_core::NodeId::from_bytes([7; 16]).to_string();
        for (arguments, expected) in [
            (vec!["diagnostics", "show"], 5),
            (vec!["peer", "trust", peer.as_str()], 4),
            (vec!["network", "list"], 1),
        ] {
            let result = Command::new(env!("CARGO_BIN_EXE_blueroute"))
                .env("DBUS_SYSTEM_BUS_ADDRESS", &broker_address)
                .args(&arguments)
                .output()?;
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(
                result.status.code(),
                Some(expected),
                "args={arguments:?}; stderr={stderr}"
            );
            assert!(result.stdout.is_empty());
        }
        Ok(())
    })
}

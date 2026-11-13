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
    let json: serde_json::Value = serde_json::from_slice(&result.stderr)
        .expect("--json failure must be valid JSON on stderr");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], 3);
    assert_eq!(json["error"]["kind"], "daemon_unavailable");
    assert!(
        json["error"]["help"]
            .as_str()
            .unwrap()
            .contains("TROUBLESHOOTING.md")
    );
}

#[test]
fn usage_error_is_valid_json_with_exit_two() {
    let result = invoke(&["--json", "--unrecognized"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], 2);
    assert_eq!(json["error"]["kind"], "usage");
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
                Some("get_diagnostics") => Err(fdo::Error::NotSupported(
                    "mock platform capability unavailable".into(),
                )),
                Some("trust_peer") => Err(fdo::Error::AccessDenied(
                    "mock PolicyKit authorization denied".into(),
                )),
                Some("list_networks") => {
                    // Spoofed diagnostic TEXT must not be treated as a typed AccessDenied.
                    Err(fdo::Error::Failed(
                        "AccessDenied string is not an auth error".into(),
                    ))
                }
                Some("get_status") => Err(fdo::Error::Failed("mock status failure".into())),
                Some("get_capabilities") => Err(fdo::Error::NotSupported(
                    "mock capability unavailable".into(),
                )),
                // Production join is refused as unsupported until authenticated transport exists.
                Some("join_network") => {
                    Err(fdo::Error::NotSupported("mock join unavailable".into()))
                }
                // Successful mutations and reads.
                Some("create_network") => Ok(r#"{"type":"ack"}"#.into()),
                Some("get_client_link") => Ok(r#"{"type":"client_link","data":null}"#.into()),
                // Typed refusals on the other mutations.
                Some("enroll_peer_key") => Err(fdo::Error::AccessDenied(
                    "mock PolicyKit authorization denied".into(),
                )),
                Some("retry_client_reconnect") => Err(fdo::Error::AccessDenied(
                    "mock PolicyKit authorization denied".into(),
                )),
                Some("leave_network") => {
                    Err(fdo::Error::NotSupported("mock leave unsupported".into()))
                }
                Some("forget_peer") => Err(fdo::Error::Failed("mock operational failure".into())),
                // A well-formed answer of the wrong kind is an operational failure, not success.
                Some("start_discovery") => Ok(r#"{"type":"client_link","data":null}"#.into()),
                // A missing record is reported, not printed as an empty success.
                Some("get_node") => Ok(r#"{"type":"node","data":null}"#.into()),
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
        let network = blueroute_core::NetworkId::from_bytes([8; 16]).to_string();
        let key = "0a".repeat(32);
        let run = |arguments: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_blueroute"))
                .env("DBUS_SYSTEM_BUS_ADDRESS", &broker_address)
                .args(arguments)
                .output()
        };
        // Failures: the exit code follows the typed D-Bus error, and nothing goes to stdout.
        for (arguments, expected) in [
            (vec!["diagnostics", "show"], 5),
            (vec!["peer", "trust", peer.as_str()], 4),
            (vec!["network", "list"], 1),
            (vec!["network", "reconnect"], 4),
            (vec!["peer", "enroll", peer.as_str(), key.as_str()], 4),
            (vec!["network", "leave"], 5),
            (vec!["peer", "forget", peer.as_str()], 1),
            (vec!["discover", "start"], 1),
            (vec!["node", "show", peer.as_str()], 1),
            (vec!["status"], 1),
            (vec!["capability", "show"], 5),
            (vec!["network", "join", network.as_str()], 5),
        ] {
            let result = run(&arguments)?;
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(
                result.status.code(),
                Some(expected),
                "args={arguments:?}; stderr={stderr}"
            );
            assert!(result.stdout.is_empty(), "args={arguments:?}");
        }

        // The JSON failure envelope carries the same typed category.
        let denied = run(&["--json", "network", "reconnect"])?;
        assert_eq!(denied.status.code(), Some(4));
        let envelope: serde_json::Value = serde_json::from_slice(&denied.stderr)?;
        assert_eq!(envelope["error"]["code"], 4);

        // Successes exit 0 with their output on stdout only.
        for (arguments, stdout) in [
            (vec!["network", "create", "Lab"], "Network created"),
            (vec!["--json", "network", "create", "Lab"], r#"{"ok":true}"#),
            (vec!["network", "link"], "not a joined client"),
            (vec!["--json", "network", "link"], "null"),
        ] {
            let result = run(&arguments)?;
            let text = String::from_utf8_lossy(&result.stdout);
            assert_eq!(result.status.code(), Some(0), "args={arguments:?}");
            assert!(text.contains(stdout), "args={arguments:?}; stdout={text}");
            assert!(result.stderr.is_empty(), "args={arguments:?}");
        }
        Ok(())
    })
}

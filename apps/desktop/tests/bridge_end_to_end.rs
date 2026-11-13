//! P12-012: the real desktop bridge and shared client against a scripted fake daemon on a private
//! D-Bus, with no Bluetooth. Each command the daemon receives is recorded, so the test also checks
//! what the bridge does *not* send.
use std::error::Error;
use std::sync::{Arc, Mutex};

use blueroute_client::BlueRouteClient;
use blueroute_core::{
    CapabilitySource, DisplayName, HealthLevel, NetworkId, NodeCapabilities, NodeId, Sourced,
};
use blueroute_desktop::{ActionAvailability, DesktopBridge, DesktopError, PlatformIssue};
use blueroute_protocol::{
    ApiVersion, Command, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus, NetworkSummary,
    Response, decode_command, encode_response,
};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::{Connection, fdo, interface};

/// Answers like a daemon on a computer whose Bluetooth adapter is off.
struct FakeDaemon {
    received: Arc<Mutex<Vec<Command>>>,
}

fn reply(response: &Response) -> fdo::Result<String> {
    encode_response(response).map_err(|_| fdo::Error::Failed("fixture encode".into()))
}

#[interface(name = "org.blueroute.Service1")]
impl FakeDaemon {
    fn version(&self) -> (u16, u16) {
        (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor)
    }

    fn request(&self, payload: &str) -> fdo::Result<String> {
        let command = decode_command(payload)
            .map_err(|_| fdo::Error::InvalidArgs("malformed fixture request".into()))?;
        self.received.lock().unwrap().push(command.clone());
        match command {
            Command::GetStatus => reply(&Response::Status(DaemonStatus {
                api_version: ApiVersion::CURRENT,
                local_node: Some(NodeId::from_bytes([1; 16])),
                current_network: None,
                health: HealthLevel::Healthy,
                capabilities: NodeCapabilities {
                    adapter_usable: Some(Sourced::new(false, CapabilitySource::Discovered)),
                    ..NodeCapabilities::default()
                },
            })),
            Command::ListNetworks => reply(&Response::Networks(vec![NetworkSummary {
                id: NetworkId::from_bytes([2; 16]),
                name: DisplayName::new("Lab").unwrap(),
                member_count: 1,
            }])),
            // A well-formed answer of the wrong kind.
            Command::ListNodes => reply(&Response::Ack),
            Command::GetClientLink => reply(&Response::ClientLink(None)),
            Command::CreateNetwork { .. } => reply(&Response::Ack),
            Command::LeaveNetwork => Err(fdo::Error::AccessDenied("mock PolicyKit denial".into())),
            Command::SetDeviceName { .. } => {
                Err(fdo::Error::NotSupported("mock rename unsupported".into()))
            }
            Command::RetryClientReconnect => Err(fdo::Error::Failed("mock failure".into())),
            _ => Err(fdo::Error::InvalidArgs("unexpected fixture command".into())),
        }
    }
}

#[test]
#[ignore = "requires an isolated D-Bus session; CI runs this under dbus-run-session"]
fn desktop_bridge_end_to_end_over_a_private_bus() -> Result<(), Box<dyn Error>> {
    future::block_on(async {
        let received = Arc::new(Mutex::new(Vec::new()));
        let server = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(
                DBUS_OBJECT_PATH,
                FakeDaemon {
                    received: received.clone(),
                },
            )?
            .build()
            .await?;
        let connect = || async {
            let connection = Connection::session().await?;
            Ok::<_, Box<dyn Error>>(DesktopBridge::with_client(
                BlueRouteClient::from_connection(connection).await?,
            ))
        };
        let bridge = connect().await?;

        // Capability-limited state: adapter off, so nothing is offered and join stays closed.
        let check = bridge
            .platform_check()
            .await
            .map_err(|e| format!("{e:?}"))?;
        assert!(
            check
                .issues
                .contains(&PlatformIssue::AdapterMissingOrDisabled)
        );
        assert_eq!(check.create, ActionAvailability::Blocked);
        assert!(!check.join_enabled);
        assert!(!check.internet_sharing_enabled);
        let overview = bridge.overview().await.map_err(|e| format!("{e:?}"))?;
        assert_eq!(overview.adapter_available, Some(false));
        assert_eq!(overview.authenticated_remote_peers, None);
        assert!(!overview.internet_sharing_available);

        let networks = bridge.networks().await.map_err(|e| format!("{e:?}"))?;
        assert_eq!(networks[0].name.as_str(), "Lab");
        assert_eq!(bridge.client_link().await, Ok(None));
        // A reply of the wrong kind is never shown as data.
        assert_eq!(
            bridge.devices().await,
            Err(DesktopError::UnexpectedResponse)
        );

        // Mutations: the trimmed name reaches the daemon; typed refusals map to typed errors.
        assert_eq!(bridge.create_network("  Workshop ").await, Ok(()));
        assert_eq!(
            bridge.leave_network().await,
            Err(DesktopError::PermissionDenied)
        );
        assert_eq!(
            bridge.set_device_name("Kitchen").await,
            Err(DesktopError::Unsupported)
        );
        assert_eq!(
            bridge.retry_client_reconnect().await,
            Err(DesktopError::Operational)
        );

        // Invalid names are refused locally and never reach the daemon.
        let before = received.lock().unwrap().len();
        assert_eq!(
            bridge.create_network("bad\nname").await,
            Err(DesktopError::InvalidName)
        );
        assert_eq!(
            bridge.set_device_name("   ").await,
            Err(DesktopError::InvalidName)
        );
        assert_eq!(received.lock().unwrap().len(), before);

        let commands = received.lock().unwrap().clone();
        assert!(commands.contains(&Command::CreateNetwork {
            name: DisplayName::new("Workshop").unwrap()
        }));
        // The bridge only ever sends its fixed commands.
        assert!(commands.iter().all(|command| matches!(
            command,
            Command::GetStatus
                | Command::ListNetworks
                | Command::ListNodes
                | Command::GetClientLink
                | Command::CreateNetwork { .. }
                | Command::LeaveNetwork
                | Command::SetDeviceName { .. }
                | Command::RetryClientReconnect
        )));

        // The daemon goes away: the next read reports it unavailable, never stale data.
        drop(server);
        assert_eq!(
            bridge.overview().await.err(),
            Some(DesktopError::DaemonUnavailable)
        );
        assert!(connect().await.is_err());
        Ok(())
    })
}

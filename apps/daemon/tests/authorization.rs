use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use blueroute_core::{
    DisplayName, HealthLevel, NetworkId, NodeCapabilities, NodeId,
};
use blueroute_daemon::{
    DaemonService, INTERNET_SHARING_ACTION_ID, MODIFY_ACTION_ID, NetworkOperationFuture,
    NetworkOperations,
};
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus,
    NetworkSummary, Response, decode_response, encode_command,
};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::interface;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, Proxy};

const POLICYKIT_SERVICE: &str = "org.freedesktop.PolicyKit1";
const POLICYKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";

#[derive(Clone)]
struct MockAuthority {
    authorized: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    last_action: Arc<Mutex<Option<String>>>,
    last_subject_kind: Arc<Mutex<Option<String>>>,
    last_flags: Arc<Mutex<Option<u32>>>,
}

#[interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl MockAuthority {
    fn check_authorization(
        &self,
        subject: (String, HashMap<String, OwnedValue>),
        action_id: &str,
        _details: HashMap<String, String>,
        flags: u32,
        _cancellation_id: &str,
    ) -> (bool, bool, HashMap<String, String>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_action.lock().unwrap() = Some(action_id.to_owned());
        *self.last_subject_kind.lock().unwrap() = Some(subject.0);
        *self.last_flags.lock().unwrap() = Some(flags);
        assert!(
            subject.1.contains_key("name"),
            "PolicyKit subject must identify the original D-Bus sender"
        );
        (
            self.authorized.load(Ordering::SeqCst),
            false,
            HashMap::new(),
        )
    }
}

#[derive(Clone)]
struct FakeNetworkOperations {
    created_network: NetworkId,
    create_calls: Arc<AtomicUsize>,
}

impl NetworkOperations for FakeNetworkOperations {
    fn create_network(&self, _name: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        Box::pin(async move {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.created_network)
        })
    }

    fn list_networks(&self) -> Result<Vec<NetworkSummary>, blueroute_core::CoreError> {
        Ok(Vec::new())
    }
}

#[test]
#[ignore = "requires an isolated D-Bus session; CI runs this under dbus-run-session"]
fn authorization_is_read_only_by_default_and_polkit_gates_mutations() -> Result<(), Box<dyn Error>>
{
    future::block_on(async {
        let authorized = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let last_action = Arc::new(Mutex::new(None));
        let last_subject_kind = Arc::new(Mutex::new(None));
        let last_flags = Arc::new(Mutex::new(None));
        let authority = Builder::session()?
            .name(POLICYKIT_SERVICE)?
            .serve_at(
                POLICYKIT_PATH,
                MockAuthority {
                    authorized: authorized.clone(),
                    calls: calls.clone(),
                    last_action: last_action.clone(),
                    last_subject_kind: last_subject_kind.clone(),
                    last_flags: last_flags.clone(),
                },
            )?
            .build()
            .await?;

        let created_network = NetworkId::from_bytes([8; 16]);
        let create_calls = Arc::new(AtomicUsize::new(0));
        let service = DaemonService::with_network_operations(
            DaemonStatus {
                api_version: ApiVersion::CURRENT,
                local_node: Some(NodeId::from_bytes([7; 16])),
                current_network: None,
                health: HealthLevel::Healthy,
                capabilities: NodeCapabilities::default(),
            },
            Arc::new(FakeNetworkOperations {
                created_network,
                create_calls: create_calls.clone(),
            }),
        );
        let _server = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, service)?
            .build()
            .await?;
        let client = Connection::session().await?;
        let proxy = Proxy::new(
            &client,
            DBUS_SERVICE_NAME,
            DBUS_OBJECT_PATH,
            DBUS_INTERFACE_NAME,
        )
        .await?;

        let get_status = encode_command(&Command::GetStatus)?;
        let payload: String = proxy.call("Request", &(get_status.clone(),)).await?;
        assert!(matches!(decode_response(&payload)?, Response::Status(_)));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "read-only requests must not contact PolicyKit"
        );

        let malformed: zbus::Result<String> = proxy.call("Request", &("{",)).await;
        assert!(malformed.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "malformed requests must fail before authorization"
        );

        let create_network = encode_command(&Command::CreateNetwork {
            name: DisplayName::new("Authorization test").unwrap(),
        })?;
        let denied: zbus::Result<String> = proxy.call("Request", &(create_network.clone(),)).await;
        let denied = denied.expect_err("unauthorized mutation must fail closed");
        assert!(denied.to_string().contains("AccessDenied"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            last_action.lock().unwrap().as_deref(),
            Some(MODIFY_ACTION_ID)
        );
        assert_eq!(
            last_subject_kind.lock().unwrap().as_deref(),
            Some("system-bus-name")
        );
        assert_eq!(*last_flags.lock().unwrap(), Some(1));

        authorized.store(true, Ordering::SeqCst);
        let payload: String = proxy.call("Request", &(create_network,)).await?;
        assert_eq!(decode_response(&payload)?, Response::Ack);
        assert_eq!(create_calls.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let payload: String = proxy.call("Request", &(get_status,)).await?;
        let Response::Status(status) = decode_response(&payload)? else {
            panic!("GetStatus must return a status response");
        };
        assert_eq!(status.current_network, Some(created_network));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "status inspection after creation must remain read-only"
        );

        let internet = encode_command(&Command::SetInternetSharing { enabled: true })?;
        let reserved: zbus::Result<String> = proxy.call("Request", &(internet,)).await;
        let error = reserved.expect_err("reserved gateway command remains unimplemented");
        assert!(error.to_string().contains("NotSupported"));
        assert_eq!(
            last_action.lock().unwrap().as_deref(),
            Some(INTERNET_SHARING_ACTION_ID)
        );

        assert!(authority.release_name(POLICYKIT_SERVICE).await?);
        authority.close().await?;
        authorized.store(false, Ordering::SeqCst);
        let unavailable = encode_command(&Command::StartDiscovery)?;
        let error: zbus::Result<String> = proxy.call("Request", &(unavailable,)).await;
        assert!(
            error
                .expect_err("missing PolicyKit must fail closed")
                .to_string()
                .contains("AccessDenied")
        );

        Ok(())
    })
}

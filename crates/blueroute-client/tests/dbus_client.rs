use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_io::Timer;
use blueroute_client::{BlueRouteClient, ClientError};
use blueroute_core::{HealthLevel, NodeCapabilities, NodeId};
use blueroute_daemon::{DaemonService, emit_event};
use blueroute_protocol::{ApiVersion, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, Event};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::fdo;
use zbus::interface;
use zbus::Connection;

#[derive(Clone)]
struct IncompatibleService {
    requests: Arc<AtomicUsize>,
}

#[interface(name = "org.blueroute.Service1")]
impl IncompatibleService {
    fn version(&self) -> (u16, u16) {
        (2, 0)
    }

    fn request(&self, _payload: &str) -> fdo::Result<String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Err(fdo::Error::Failed(
            "incompatible test service must never receive a normal request".into(),
        ))
    }
}

#[test]
#[ignore = "requires an isolated D-Bus session; CI runs this under dbus-run-session"]
fn client_queries_events_reconnects_and_fails_closed_on_incompatibility() -> Result<(), Box<dyn Error>> {
    future::block_on(async {
        let first_node = NodeId::from_bytes([1; 16]);
        let first_server = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(
                DBUS_OBJECT_PATH,
                DaemonService::new(
                    first_node,
                    HealthLevel::Healthy,
                    NodeCapabilities::default(),
                ),
            )?
            .build()
            .await?;

        let client_connection = Connection::session().await?;
        let mut client = BlueRouteClient::from_connection(client_connection.clone()).await?;
        assert_eq!(client.server_version(), ApiVersion::CURRENT);

        let status = client.status().await?;
        assert_eq!(status.local_node, Some(first_node));
        assert_eq!(status.health, HealthLevel::Healthy);
        assert_eq!(client.capabilities().await?, NodeCapabilities::default());

        let mut events = client.events().await?;
        let expected_event = Event::HealthChanged(HealthLevel::Degraded);
        emit_event(&first_server, &expected_event).await?;
        let received = future::or(events.next_event(), async {
            Timer::after(Duration::from_secs(3)).await;
            Err(ClientError::EventStreamClosed)
        })
        .await?;
        assert_eq!(received, expected_event);
        drop(events);

        assert!(first_server.release_name(DBUS_SERVICE_NAME).await?);
        first_server.close().await?;

        let second_node = NodeId::from_bytes([2; 16]);
        let second_server = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(
                DBUS_OBJECT_PATH,
                DaemonService::new(
                    second_node,
                    HealthLevel::Degraded,
                    NodeCapabilities::default(),
                ),
            )?
            .build()
            .await?;
        client.reconnect(Duration::from_secs(3)).await?;
        let status = client.status().await?;
        assert_eq!(status.local_node, Some(second_node));
        assert_eq!(status.health, HealthLevel::Degraded);

        assert!(second_server.release_name(DBUS_SERVICE_NAME).await?);
        second_server.close().await?;

        let request_count = Arc::new(AtomicUsize::new(0));
        let incompatible_server = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(
                DBUS_OBJECT_PATH,
                IncompatibleService {
                    requests: request_count.clone(),
                },
            )?
            .build()
            .await?;

        let error = client.status().await.unwrap_err();
        assert!(matches!(
            error,
            ClientError::IncompatibleVersion {
                client: ApiVersion { major: 1, .. },
                server: ApiVersion { major: 2, minor: 0 },
            }
        ));
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            0,
            "client must reject an incompatible daemon before sending Request"
        );

        assert!(incompatible_server.release_name(DBUS_SERVICE_NAME).await?);
        incompatible_server.close().await?;
        Ok(())
    })
}

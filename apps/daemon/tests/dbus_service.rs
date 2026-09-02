use std::error::Error;
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{HealthLevel, NodeCapabilities, NodeId};
use blueroute_daemon::{DaemonService, emit_event};
use blueroute_protocol::{
    ApiVersion, Event, Response, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME,
    decode_event, decode_response,
};
use futures_lite::{StreamExt, future};
use zbus::connection::Builder;
use zbus::{Connection, Proxy};

#[test]
#[ignore = "requires an isolated D-Bus session; CI runs this under dbus-run-session"]
fn dbus_service_round_trip_and_event() -> Result<(), Box<dyn Error>> {
    future::block_on(async {
        let local_node = NodeId::from_bytes([7; 16]);
        let service = DaemonService::new(
            local_node,
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        );
        let server = Builder::session()?
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

        let version: (u16, u16) = proxy.call("Version", &()).await?;
        assert_eq!(version, (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor));

        let status_payload: String = proxy.call("Status", &()).await?;
        let Response::Status(status) = decode_response(&status_payload)? else {
            panic!("Status D-Bus method returned the wrong protocol response variant");
        };
        assert_eq!(status.api_version, ApiVersion::CURRENT);
        assert_eq!(status.local_node, Some(local_node));
        assert_eq!(status.health, HealthLevel::Healthy);

        let capabilities_payload: String = proxy.call("Capabilities", &()).await?;
        let Response::Capabilities(capabilities) = decode_response(&capabilities_payload)? else {
            panic!("Capabilities D-Bus method returned the wrong protocol response variant");
        };
        assert_eq!(capabilities, NodeCapabilities::default());

        let malformed: zbus::Result<String> = proxy.call("Request", &("{",)).await;
        assert!(malformed.is_err(), "malformed requests must fail closed");

        let mut events = proxy.receive_signal("Event").await?;
        let expected = Event::HealthChanged(HealthLevel::Degraded);
        emit_event(&server, &expected).await?;
        let message = future::or(
            events.next(),
            async {
                Timer::after(Duration::from_secs(3)).await;
                None
            },
        )
        .await
        .ok_or("timed out waiting for the daemon event signal")?;
        let payload: String = message.body().deserialize()?;
        assert_eq!(decode_event(&payload)?, expected);

        Ok(())
    })
}

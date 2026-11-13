//! CR-F1 regressions (docs/CODE_REVIEW_REMEDIATION_TODO.md, EVENT-1/EVENT-6).
//!
//! A committed daemon state change must reach an already connected subscriber, without anything
//! calling `emit_event` by hand. These tests state the corrected contract and fail on baseline
//! 4ca5042, where state changes update the snapshot but emit no signal. They need a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-daemon --test state_events --locked -- --ignored`
use std::error::Error;
use std::time::Duration;

use async_io::Timer;
use blueroute_core::{HealthLevel, NodeCapabilities, NodeId};
use blueroute_daemon::DaemonService;
use blueroute_protocol::{DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME};
use futures_lite::{StreamExt, future};
use zbus::connection::Builder;
use zbus::{Connection, Proxy};

/// EVENT-6: a published change reaches a healthy connected subscriber within this bound.
const DELIVERY_BOUND: Duration = Duration::from_secs(1);

#[test]
#[ignore = "CR-F1 regression: fails on baseline 4ca5042 until CR-F1 is fixed; needs dbus-run-session"]
fn cr_f1_a_worker_health_change_reaches_a_connected_subscriber() -> Result<(), Box<dyn Error>> {
    future::block_on(async {
        let service = DaemonService::new(
            NodeId::from_bytes([0xf1; 16]),
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        );
        let _daemon = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(DBUS_OBJECT_PATH, service.clone())?
            .build()
            .await?;
        let observer = Connection::session().await?;
        let proxy = Proxy::new(
            &observer,
            DBUS_SERVICE_NAME,
            DBUS_OBJECT_PATH,
            DBUS_INTERFACE_NAME,
        )
        .await?;
        let mut events = proxy.receive_signal("Event").await?;

        // The runtime worker's real path: a new health observation is published.
        assert!(service.update_runtime_observation(
            HealthLevel::Degraded,
            NodeCapabilities::default(),
            None
        )?);
        assert_eq!(service.status_snapshot()?.health, HealthLevel::Degraded);

        let outcome = future::or(async { Some(events.next().await) }, async {
            Timer::after(DELIVERY_BOUND).await;
            None
        })
        .await;
        match outcome {
            Some(Some(_)) => Ok(()),
            Some(None) => panic!("the signal stream closed"),
            None => panic!("no event reached the subscriber within {DELIVERY_BOUND:?}"),
        }
    })
}

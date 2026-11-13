//! CR-F1 regressions (docs/CODE_REVIEW_REMEDIATION_TODO.md, EVENT-1..EVENT-6).
//!
//! Real daemon state changes, made through the service's own request and worker paths, must reach
//! already connected observers as the `StateChanged` signal, emitted by the production publisher.
//! Nothing here calls an emit function by hand. On baseline 4ca5042 no signal was ever emitted.
//! They need a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-daemon --test state_events --locked -- --ignored`
use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_io::Timer;
use blueroute_core::{DisplayName, HealthLevel, NetworkId, NodeCapabilities, NodeId};
use blueroute_daemon::{
    DaemonService, NetworkOperationFuture, NetworkOperations, StatePublisherWorker,
    spawn_state_publisher,
};
use blueroute_protocol::{
    ApiVersion, Command, DBUS_INTERFACE_NAME, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME, DaemonStatus,
    NetworkSummary, Response, STATE_CHANGED_SIGNAL, decode_response, encode_command,
};
use futures_lite::{StreamExt, future};
use zbus::connection::Builder;
use zbus::proxy::SignalStream;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, Proxy, interface};

/// EVENT-6: a published change reaches a healthy connected subscriber within this bound.
const DELIVERY_BOUND: Duration = Duration::from_secs(1);
/// Long enough to be sure that nothing more is coming after a change was announced.
const QUIET: Duration = Duration::from_millis(700);

/// The tests share the daemon's and PolicyKit's names on one private bus.
static BUS: Mutex<()> = Mutex::new(());

const POLICYKIT_SERVICE: &str = "org.freedesktop.PolicyKit1";
const POLICYKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";

struct Authority {
    allow: Arc<AtomicBool>,
}

#[interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl Authority {
    fn check_authorization(
        &self,
        _subject: (String, HashMap<String, OwnedValue>),
        _action_id: &str,
        _details: HashMap<String, String>,
        _flags: u32,
        _cancellation_id: &str,
    ) -> (bool, bool, HashMap<String, String>) {
        (self.allow.load(Ordering::SeqCst), false, HashMap::new())
    }
}

/// Networks kept in memory: create commits one, leave removes it.
#[derive(Default)]
struct Networks {
    list: Mutex<Vec<NetworkSummary>>,
}

impl NetworkOperations for Networks {
    fn create_network(&self, name: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
        Box::pin(async move {
            let id = NetworkId::from_bytes([0x5a; 16]);
            self.list.lock().unwrap().push(NetworkSummary {
                id,
                name,
                member_count: 1,
            });
            Ok(id)
        })
    }

    fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async move {
            self.list.lock().unwrap().clear();
            Ok(())
        })
    }

    fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>> {
        Box::pin(async move { Ok(self.list.lock().unwrap().clone()) })
    }

    fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct Daemon {
    service: DaemonService,
    connection: Connection,
    allow: Arc<AtomicBool>,
    _authority: Connection,
    _publisher: StatePublisherWorker,
}

/// A production `DaemonService` served on the private bus, with its production publisher.
async fn daemon() -> Result<Daemon, Box<dyn Error>> {
    daemon_with(|service| service).await
}

async fn daemon_with(
    configure: impl FnOnce(DaemonService) -> DaemonService,
) -> Result<Daemon, Box<dyn Error>> {
    let allow = Arc::new(AtomicBool::new(true));
    let authority = Builder::session()?
        .name(POLICYKIT_SERVICE)?
        .serve_at(
            POLICYKIT_PATH,
            Authority {
                allow: allow.clone(),
            },
        )?
        .build()
        .await?;
    let service = configure(DaemonService::with_network_operations(
        DaemonStatus {
            api_version: ApiVersion::CURRENT,
            local_node: Some(NodeId::from_bytes([0xf1; 16])),
            current_network: None,
            health: HealthLevel::Healthy,
            capabilities: NodeCapabilities::default(),
        },
        Arc::new(Networks::default()),
    ));
    let connection = Builder::session()?
        .name(DBUS_SERVICE_NAME)?
        .serve_at(DBUS_OBJECT_PATH, service.clone())?
        .build()
        .await?;
    let publisher = spawn_state_publisher(connection.clone(), service.clone())?;
    Ok(Daemon {
        service,
        connection,
        allow,
        _authority: authority,
        _publisher: publisher,
    })
}

/// A separate bus client: what another front end would be.
struct Observer {
    proxy: Proxy<'static>,
    signals: SignalStream<'static>,
}

async fn observer() -> Result<Observer, Box<dyn Error>> {
    let connection = Connection::session().await?;
    let proxy = Proxy::new_owned(
        connection,
        DBUS_SERVICE_NAME,
        DBUS_OBJECT_PATH,
        DBUS_INTERFACE_NAME,
    )
    .await?;
    let signals = proxy.receive_signal(STATE_CHANGED_SIGNAL).await?;
    Ok(Observer { proxy, signals })
}

impl Observer {
    /// The next revision within `bound`, or `None`.
    async fn next_within(&mut self, bound: Duration) -> Option<u64> {
        let signals = &mut self.signals;
        future::or(
            async {
                let message = signals.next().await?;
                message.body().deserialize::<u64>().ok()
            },
            async {
                Timer::after(bound).await;
                None
            },
        )
        .await
    }

    async fn request(&self, command: &Command) -> zbus::Result<Response> {
        let payload: String = self
            .proxy
            .call(
                "Request",
                &(encode_command(command).expect("a valid command"),),
            )
            .await?;
        Ok(decode_response(&payload).expect("a valid response"))
    }

    /// Wait until nothing more arrives for `QUIET`; how many arrived meanwhile.
    async fn drain(&mut self) -> usize {
        let mut count = 0;
        while self.next_within(QUIET).await.is_some() {
            count += 1;
        }
        count
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    BUS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn name(text: &str) -> DisplayName {
    DisplayName::new(text).unwrap()
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_a_worker_health_change_reaches_a_connected_subscriber() -> Result<(), Box<dyn Error>> {
    let _bus = lock();
    future::block_on(async {
        let daemon = daemon().await?;
        let mut observer = observer().await?;

        // The runtime worker's real path: a new health observation is published.
        let published = Instant::now();
        assert!(daemon.service.update_runtime_observation(
            HealthLevel::Degraded,
            NodeCapabilities::default(),
            None
        )?);
        observer
            .next_within(DELIVERY_BOUND)
            .await
            .ok_or("no StateChanged within 1s of a health change")?;
        eprintln!("health change delivered in {:?}", published.elapsed());
        let Response::Status(status) = observer.request(&Command::GetStatus).await? else {
            panic!("status expected");
        };
        assert_eq!(status.health, HealthLevel::Degraded);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_another_clients_committed_create_and_leave_reach_every_observer()
-> Result<(), Box<dyn Error>> {
    let _bus = lock();
    future::block_on(async {
        let daemon = daemon().await?;
        let mut first = observer().await?;
        let mut second = observer().await?;
        let actor = observer().await?;

        let committed = Instant::now();
        actor
            .request(&Command::CreateNetwork { name: name("Lab") })
            .await?;
        for observer in [&mut first, &mut second] {
            observer
                .next_within(DELIVERY_BOUND)
                .await
                .ok_or("an observer missed the committed create")?;
        }
        eprintln!("create delivered in {:?}", committed.elapsed());
        // The reread shows the committed state.
        let Response::Networks(networks) = first.request(&Command::ListNetworks).await? else {
            panic!("networks expected");
        };
        assert_eq!(networks.len(), 1);
        let Response::Status(status) = second.request(&Command::GetStatus).await? else {
            panic!("status expected");
        };
        assert!(status.current_network.is_some());
        first.drain().await;
        second.drain().await;

        actor.request(&Command::LeaveNetwork).await?;
        for observer in [&mut first, &mut second] {
            observer
                .next_within(DELIVERY_BOUND)
                .await
                .ok_or("an observer missed the committed leave")?;
        }
        let Response::Networks(networks) = first.request(&Command::ListNetworks).await? else {
            panic!("networks expected");
        };
        assert!(networks.is_empty());
        drop(daemon);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_a_rename_is_announced() -> Result<(), Box<dyn Error>> {
    let _bus = lock();
    future::block_on(async {
        let state = tempfile_dir()?;
        let store = Arc::new(blueroute_daemon::DeviceNameStore::new(state.join("name")));
        let _daemon =
            daemon_with(|service| service.with_device_name_store(store, name("Before"))).await?;
        let mut observer = observer().await?;
        observer
            .request(&Command::SetDeviceName {
                name: name("After"),
            })
            .await?;
        observer
            .next_within(DELIVERY_BOUND)
            .await
            .ok_or("no StateChanged after a rename")?;
        let Response::Nodes(nodes) = observer.request(&Command::ListNodes).await? else {
            panic!("nodes expected");
        };
        assert_eq!(nodes[0].name.as_str(), "After");
        std::fs::remove_dir_all(state).ok();
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_a_refused_mutation_announces_no_success() -> Result<(), Box<dyn Error>> {
    let _bus = lock();
    future::block_on(async {
        let daemon = daemon().await?;
        let mut observer = observer().await?;
        daemon.allow.store(false, Ordering::SeqCst);
        let refused = observer
            .request(&Command::CreateNetwork { name: name("Lab") })
            .await;
        assert!(refused.is_err());
        // The failure is recorded in diagnostics, so observers are told to reread …
        observer
            .next_within(DELIVERY_BOUND)
            .await
            .ok_or("the recorded failure was not announced")?;
        // … and what they reread shows no network: no false success.
        let Response::Status(status) = observer.request(&Command::GetStatus).await? else {
            panic!("status expected");
        };
        assert_eq!(status.current_network, None);
        let Response::Diagnostics(diagnostics) = observer.request(&Command::GetDiagnostics).await?
        else {
            panic!("diagnostics expected");
        };
        assert_eq!(diagnostics.recent_errors.len(), 1);
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_unchanged_observations_and_reads_announce_nothing() -> Result<(), Box<dyn Error>> {
    let _bus = lock();
    future::block_on(async {
        let daemon = daemon().await?;
        let mut observer = observer().await?;
        for _ in 0..5 {
            assert!(!daemon.service.update_runtime_observation(
                HealthLevel::Healthy,
                NodeCapabilities::default(),
                None
            )?);
            observer.request(&Command::GetStatus).await?;
            observer.request(&Command::ListNetworks).await?;
        }
        assert_eq!(
            observer.drain().await,
            0,
            "an unchanged state was announced"
        );
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_a_burst_coalesces_and_the_final_state_is_announced() -> Result<(), Box<dyn Error>> {
    let _bus = lock();
    future::block_on(async {
        let daemon = daemon().await?;
        let mut observer = observer().await?;
        let levels = [HealthLevel::Degraded, HealthLevel::Error];
        for round in 0..200 {
            daemon.service.update_runtime_observation(
                levels[round % 2],
                NodeCapabilities::default(),
                None,
            )?;
        }
        // The burst ended on Error (round 199). However the signals coalesced, at least one
        // arrives after the last change, so a reread shows it.
        let received = observer.drain().await;
        assert!(received >= 1, "the burst was never announced");
        assert!(received < 200, "nothing was coalesced ({received} signals)");
        let Response::Status(status) = observer.request(&Command::GetStatus).await? else {
            panic!("status expected");
        };
        assert_eq!(status.health, HealthLevel::Error);
        eprintln!("200 changes coalesced into {received} signals");
        Ok(())
    })
}

#[test]
#[ignore = "needs a private bus: run with dbus-run-session"]
fn cr_f1_an_emission_failure_keeps_the_change_pending_and_never_fails_the_mutation()
-> Result<(), Box<dyn Error>> {
    let _bus = lock();
    future::block_on(async {
        let daemon = daemon().await?;
        let mut observer = observer().await?;
        // Take the interface off the bus object, so emitting the signal fails.
        let server = daemon.connection.object_server();
        server.remove::<DaemonService, _>(DBUS_OBJECT_PATH).await?;

        // The change itself succeeds regardless.
        assert!(daemon.service.update_runtime_observation(
            HealthLevel::Error,
            NodeCapabilities::default(),
            None
        )?);
        assert_eq!(observer.next_within(QUIET).await, None);

        // Once emitting works again, the pending invalidation goes out without a new change.
        server.at(DBUS_OBJECT_PATH, daemon.service.clone()).await?;
        observer
            .next_within(DELIVERY_BOUND)
            .await
            .ok_or("the pending change was dropped after the emission failure")?;
        Ok(())
    })
}

/// A unique temporary directory, without a tempfile dependency.
fn tempfile_dir() -> Result<std::path::PathBuf, Box<dyn Error>> {
    let path = std::env::temp_dir().join(format!(
        "blueroute-state-events-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

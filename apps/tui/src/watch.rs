//! Daemon change signals for the TUI. Event payloads are dropped: the UI only learns that the
//! daemon link came up, went away, or that something changed, and then re-reads the typed
//! snapshot. The loop is [`blueroute_client::watch`], shared with the desktop: an idle daemon that
//! goes away or is replaced ends the subscription without any Event, and the link is retried
//! with bounded exponential backoff (1 s doubling to 30 s).
use std::io;
use std::ops::ControlFlow;
use std::sync::mpsc::{self, Receiver};
#[cfg(test)]
use std::time::Duration;

use async_io::Timer;
use blueroute_client::{BlueRouteClient, WatchSignal};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Signal {
    Connected,
    Changed,
    Disconnected,
}

impl From<WatchSignal> for Signal {
    fn from(signal: WatchSignal) -> Self {
        match signal {
            WatchSignal::Connected => Self::Connected,
            WatchSignal::Changed => Self::Changed,
            WatchSignal::Disconnected => Self::Disconnected,
        }
    }
}

#[cfg(test)]
pub(crate) fn reconnect_delay(attempt: u32) -> Duration {
    blueroute_client::reconnect_delay(attempt)
}

/// Run the watcher on its own thread. It stops, releasing its subscription, once the receiver
/// is dropped and the next signal cannot be delivered.
pub(crate) fn spawn() -> io::Result<Receiver<Signal>> {
    spawn_with(BlueRouteClient::connect)
}

/// [`spawn`] with the connection injected, so tests can use a private bus. Everything else is
/// the production watcher.
fn spawn_with<Connect, Connecting>(connect: Connect) -> io::Result<Receiver<Signal>>
where
    Connect: FnMut() -> Connecting + Send + 'static,
    Connecting:
        std::future::Future<Output = Result<BlueRouteClient, blueroute_client::ClientError>>,
{
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("blueroute-tui-watch".to_owned())
        .spawn(move || {
            futures_lite::future::block_on(blueroute_client::watch(
                connect,
                |delay| async move {
                    Timer::after(delay).await;
                },
                |signal| match send.send(signal.into()) {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(_) => ControlFlow::Break(()),
                },
            ));
        })?;
    Ok(receive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_is_exponential_and_bounded() {
        assert_eq!(reconnect_delay(0), Duration::from_secs(1));
        assert_eq!(reconnect_delay(3), Duration::from_secs(8));
        assert_eq!(reconnect_delay(5), Duration::from_secs(30));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(30));
    }

    /// CR-INTEGRATION-01: the TUI's watcher thread, channel and stop-on-drop, with only the
    /// connection swapped for the private bus of `dbus-run-session`:
    ///
    /// `dbus-run-session -- cargo test -p blueroute-tui --locked -- --ignored tui_watcher`
    struct FakeDaemon;

    /// The bus tests share the daemon's well-known name, so they run one at a time.
    static BUS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn bus() -> std::sync::MutexGuard<'static, ()> {
        BUS.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[zbus::interface(name = "org.blueroute.Service1")]
    impl FakeDaemon {
        fn version(&self) -> (u16, u16) {
            (
                blueroute_protocol::ApiVersion::CURRENT.major,
                blueroute_protocol::ApiVersion::CURRENT.minor,
            )
        }

        #[zbus(signal)]
        async fn state_changed(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            revision: u64,
        ) -> zbus::Result<()>;
    }

    async fn daemon() -> zbus::Result<zbus::Connection> {
        zbus::connection::Builder::session()?
            .name(blueroute_protocol::DBUS_SERVICE_NAME)?
            .serve_at(blueroute_protocol::DBUS_OBJECT_PATH, FakeDaemon)?
            .build()
            .await
    }

    fn next(receive: &Receiver<Signal>, bound: Duration) -> Option<Signal> {
        receive.recv_timeout(bound).ok()
    }

    #[test]
    #[ignore = "needs a private bus: run with dbus-run-session"]
    fn tui_watcher_follows_changes_idle_loss_return_and_quit() {
        let _bus = bus();
        futures_lite::future::block_on(async {
            let first = daemon().await.unwrap();
            let receive = spawn_with(|| async {
                let connection = zbus::Connection::session()
                    .await
                    .map_err(blueroute_client::ClientError::Bus)?;
                BlueRouteClient::from_connection(connection).await
            })
            .unwrap();
            assert_eq!(
                next(&receive, Duration::from_secs(3)),
                Some(Signal::Connected)
            );

            let interface = first
                .object_server()
                .interface::<_, FakeDaemon>(blueroute_protocol::DBUS_OBJECT_PATH)
                .await
                .unwrap();
            FakeDaemon::state_changed(interface.signal_emitter(), 1)
                .await
                .unwrap();
            assert_eq!(
                next(&receive, Duration::from_secs(1)),
                Some(Signal::Changed)
            );

            // Idle loss, no event: reported within 2 s.
            first
                .release_name(blueroute_protocol::DBUS_SERVICE_NAME)
                .await
                .unwrap();
            first.close().await.unwrap();
            assert_eq!(
                next(&receive, Duration::from_secs(2)),
                Some(Signal::Disconnected)
            );

            // Return: reconnected on the production schedule (first retry after 1 s).
            let second = daemon().await.unwrap();
            assert_eq!(
                next(&receive, Duration::from_secs(5)),
                Some(Signal::Connected)
            );

            // The TUI quits: dropping the receiver stops the watcher at its next signal, which
            // releases its subscription and bus connection.
            let bus = zbus::fdo::DBusProxy::new(&second).await.unwrap();
            let connections = || async {
                bus.list_names()
                    .await
                    .unwrap()
                    .iter()
                    .filter(|name| name.starts_with(':'))
                    .count()
            };
            let before = connections().await;
            drop(receive);
            let interface = second
                .object_server()
                .interface::<_, FakeDaemon>(blueroute_protocol::DBUS_OBJECT_PATH)
                .await
                .unwrap();
            FakeDaemon::state_changed(interface.signal_emitter(), 2)
                .await
                .unwrap();
            let mut released = false;
            for _ in 0..100 {
                // Exactly the watcher's own connection is gone once it stopped.
                if connections().await == before - 1 {
                    released = true;
                    break;
                }
                async_io::Timer::after(Duration::from_millis(20)).await;
            }
            assert!(
                released,
                "the watcher kept its bus connection after the TUI quit"
            );
        });
    }

    /// Hands the name to `successor` inside its third Version call: the check of its first change
    /// signal (calls 1 and 2 are the watcher's connect and subscribe).
    struct HandoverDaemon {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        own: std::sync::Arc<std::sync::OnceLock<zbus::Connection>>,
        successor: Option<zbus::Connection>,
    }

    #[zbus::interface(name = "org.blueroute.Service1")]
    impl HandoverDaemon {
        async fn version(&self) -> (u16, u16) {
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if call == 3
                && let Some(successor) = &self.successor
            {
                let own = self.own.get().expect("own connection");
                own.release_name(blueroute_protocol::DBUS_SERVICE_NAME)
                    .await
                    .unwrap();
                successor
                    .request_name(blueroute_protocol::DBUS_SERVICE_NAME)
                    .await
                    .unwrap();
            }
            (
                blueroute_protocol::ApiVersion::CURRENT.major,
                blueroute_protocol::ApiVersion::CURRENT.minor,
            )
        }

        #[zbus(signal)]
        async fn state_changed(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            revision: u64,
        ) -> zbus::Result<()>;
    }

    async fn announce(connection: &zbus::Connection) {
        let interface = connection
            .object_server()
            .interface::<_, HandoverDaemon>(blueroute_protocol::DBUS_OBJECT_PATH)
            .await
            .unwrap();
        HandoverDaemon::state_changed(interface.signal_emitter(), 1)
            .await
            .unwrap();
    }

    /// FU-INTEGRATION-02: an event-time handover through the TUI's real watcher thread.
    #[test]
    #[ignore = "needs a private bus: run with dbus-run-session"]
    fn tui_watcher_follows_an_event_time_handover_to_the_successor() {
        let _bus = bus();
        futures_lite::future::block_on(async {
            let b = zbus::connection::Builder::session()
                .unwrap()
                .serve_at(
                    blueroute_protocol::DBUS_OBJECT_PATH,
                    HandoverDaemon {
                        calls: Default::default(),
                        own: Default::default(),
                        successor: None,
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let own: std::sync::Arc<std::sync::OnceLock<zbus::Connection>> = Default::default();
            let a = zbus::connection::Builder::session()
                .unwrap()
                .name(blueroute_protocol::DBUS_SERVICE_NAME)
                .unwrap()
                .serve_at(
                    blueroute_protocol::DBUS_OBJECT_PATH,
                    HandoverDaemon {
                        calls: Default::default(),
                        own: own.clone(),
                        successor: Some(b.clone()),
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let _ = own.set(a.clone());

            let receive = spawn_with(|| async {
                let connection = zbus::Connection::session()
                    .await
                    .map_err(blueroute_client::ClientError::Bus)?;
                BlueRouteClient::from_connection(connection).await
            })
            .unwrap();
            assert_eq!(
                next(&receive, Duration::from_secs(3)),
                Some(Signal::Connected)
            );
            announce(&a).await;
            // A's obsolete change never shows; the loss does, then B on the production backoff.
            assert_eq!(
                next(&receive, Duration::from_secs(2)),
                Some(Signal::Disconnected)
            );
            assert_eq!(
                next(&receive, Duration::from_secs(5)),
                Some(Signal::Connected)
            );
            announce(&b).await;
            assert_eq!(
                next(&receive, Duration::from_secs(1)),
                Some(Signal::Changed)
            );
        });
    }
}

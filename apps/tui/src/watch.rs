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
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("blueroute-tui-watch".to_owned())
        .spawn(move || {
            futures_lite::future::block_on(blueroute_client::watch(
                BlueRouteClient::connect,
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
}

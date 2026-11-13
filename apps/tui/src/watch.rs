//! Daemon change signals for the TUI. Event payloads are dropped: the UI only learns that the
//! daemon link came up, went away, or that something changed, and then re-reads the typed
//! snapshot. A closed stream, version mismatch or bus error counts as a lost link and is retried
//! with bounded exponential backoff (1 s doubling to 30 s), as the desktop does.
use std::io;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use async_io::Timer;
use blueroute_client::BlueRouteClient;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Signal {
    Connected,
    Changed,
    Disconnected,
}

pub(crate) fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(1)
        .saturating_mul(2u32.saturating_pow(attempt.min(16)))
        .min(Duration::from_secs(30))
}

/// Run the watcher on its own thread for the process lifetime.
pub(crate) fn spawn() -> io::Result<Receiver<Signal>> {
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("blueroute-tui-watch".to_owned())
        .spawn(move || {
            futures_lite::future::block_on(async move {
                let mut attempt: u32 = 0;
                loop {
                    if let Ok(client) = BlueRouteClient::connect().await
                        && let Ok(mut events) = client.events().await
                    {
                        attempt = 0;
                        if send.send(Signal::Connected).is_err() {
                            return;
                        }
                        while events.next_event().await.is_ok() {
                            if send.send(Signal::Changed).is_err() {
                                return;
                            }
                        }
                    }
                    if send.send(Signal::Disconnected).is_err() {
                        return;
                    }
                    Timer::after(reconnect_delay(attempt)).await;
                    attempt = attempt.saturating_add(1);
                }
            });
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

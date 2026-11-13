//! The one daemon watcher behind the desktop and TUI live views (CR-F2, WATCH-1..WATCH-4).
//!
//! Event payloads never leave this module: a front end only learns that the link came up, that
//! something changed, or that the link is lost, and then rereads its typed snapshots. Losing the
//! owner ends the subscription even when the daemon never emits an Event (see
//! [`crate::EventSubscription::next_event`]); the watcher then reports the loss and reconnects
//! with bounded exponential backoff to a freshly validated, owner-bound subscription. An
//! incompatible owner never connects, so it stays unavailable.
use std::future::Future;
use std::ops::ControlFlow;
use std::time::Duration;

use crate::{BlueRouteClient, ClientError};

const INITIAL_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);

/// A payload-free change in the daemon link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchSignal {
    /// A version-checked, owner-bound event subscription is established; reread snapshots.
    Connected,
    /// The daemon reported a change; reread snapshots. The event itself is dropped.
    Changed,
    /// The link is lost or unavailable; anything shown from before is no longer current.
    Disconnected,
}

/// Delay before reconnect attempt `attempt` (0-based): 1 s, 2 s, 4 s … capped at 30 s.
pub fn reconnect_delay(attempt: u32) -> Duration {
    INITIAL_DELAY
        .saturating_mul(2u32.saturating_pow(attempt.min(16)))
        .min(MAX_DELAY)
}

/// Watch the daemon until `notify` returns [`ControlFlow::Break`], then release the subscription
/// and return.
///
/// `connect` builds a fresh client for each attempt and `sleep` waits out a backoff delay; both
/// are injected so tests can use a private bus and a fast clock while production keeps the
/// [`reconnect_delay`] schedule. `Disconnected` is reported once per loss, not once per failed
/// attempt; the attempt count restarts after every successful connection.
pub async fn watch<Connect, Connecting, Sleep, Sleeping>(
    mut connect: Connect,
    mut sleep: Sleep,
    mut notify: impl FnMut(WatchSignal) -> ControlFlow<()>,
) where
    Connect: FnMut() -> Connecting,
    Connecting: Future<Output = Result<BlueRouteClient, ClientError>>,
    Sleep: FnMut(Duration) -> Sleeping,
    Sleeping: Future<Output = ()>,
{
    let mut attempt: u32 = 0;
    let mut reported_loss = false;
    loop {
        if let Ok(client) = connect().await
            && let Ok(mut events) = client.events().await
        {
            attempt = 0;
            reported_loss = false;
            if notify(WatchSignal::Connected).is_break() {
                return;
            }
            while events.next_event().await.is_ok() {
                if notify(WatchSignal::Changed).is_break() {
                    return;
                }
            }
        }
        if !reported_loss {
            reported_loss = true;
            if notify(WatchSignal::Disconnected).is_break() {
                return;
            }
        }
        sleep(reconnect_delay(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_is_exponential_and_bounded() {
        assert_eq!(reconnect_delay(0), Duration::from_secs(1));
        assert_eq!(reconnect_delay(1), Duration::from_secs(2));
        assert_eq!(reconnect_delay(3), Duration::from_secs(8));
        assert_eq!(reconnect_delay(4), Duration::from_secs(16));
        assert_eq!(reconnect_delay(5), Duration::from_secs(30));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(30));
    }
}

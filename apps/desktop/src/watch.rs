//! Safe daemon change forwarding for the desktop UI.
//!
//! Daemon event payloads are never forwarded to web content. The UI only learns that the
//! daemon link came up, went away, or that *something* changed, and then re-reads the typed,
//! read-only snapshots. The loop itself is [`blueroute_client::watch`], shared with the TUI: an
//! idle daemon that goes away or is replaced ends the subscription without any Event, and the
//! link is retried with bounded exponential backoff.
use std::ops::ControlFlow;
use std::time::Duration;

use async_io::Timer;
use blueroute_client::{BlueRouteClient, WatchSignal};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonSignal {
    /// A version-checked event subscription is established; snapshots should be refreshed.
    Connected,
    /// A daemon event arrived; the payload is intentionally dropped.
    Changed,
    /// The daemon link was lost; retrying in the background.
    Disconnected,
}

impl From<WatchSignal> for DaemonSignal {
    fn from(signal: WatchSignal) -> Self {
        match signal {
            WatchSignal::Connected => Self::Connected,
            WatchSignal::Changed => Self::Changed,
            WatchSignal::Disconnected => Self::Disconnected,
        }
    }
}

/// Delay before reconnect attempt number `attempt` (0-based): 1s, 2s, 4s ... capped at 30s.
pub fn reconnect_delay(attempt: u32) -> Duration {
    blueroute_client::reconnect_delay(attempt)
}

/// Watch the daemon forever, reporting coarse signals. Intended to run for the app lifetime.
pub async fn watch_daemon(mut notify: impl FnMut(DaemonSignal)) {
    blueroute_client::watch(
        BlueRouteClient::connect,
        |delay| async move {
            Timer::after(delay).await;
        },
        |signal| {
            notify(signal.into());
            ControlFlow::Continue(())
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_is_exponential_and_bounded() {
        assert_eq!(reconnect_delay(0), Duration::from_secs(1));
        assert_eq!(reconnect_delay(1), Duration::from_secs(2));
        assert_eq!(reconnect_delay(4), Duration::from_secs(16));
        assert_eq!(reconnect_delay(5), Duration::from_secs(30));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn signals_carry_no_daemon_payload() {
        for (signal, wire) in [
            (DaemonSignal::Connected, "connected"),
            (DaemonSignal::Changed, "changed"),
            (DaemonSignal::Disconnected, "disconnected"),
        ] {
            assert_eq!(serde_json::to_value(signal).unwrap(), wire);
        }
    }
}

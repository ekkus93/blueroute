//! Safe daemon change forwarding for the desktop UI.
//!
//! Daemon event payloads are never forwarded to web content. The UI only learns that the
//! daemon link came up, went away, or that *something* changed, and then re-reads the typed,
//! read-only snapshots. A closed stream, version mismatch or bus error is treated as a lost
//! link and retried with bounded exponential backoff.
use std::time::Duration;

use async_io::Timer;
use blueroute_client::BlueRouteClient;
use serde::Serialize;

const INITIAL_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);

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

/// Delay before reconnect attempt number `attempt` (0-based): 1s, 2s, 4s ... capped at 30s.
pub fn reconnect_delay(attempt: u32) -> Duration {
    INITIAL_DELAY
        .saturating_mul(2u32.saturating_pow(attempt.min(16)))
        .min(MAX_DELAY)
}

/// Watch the daemon forever, reporting coarse signals. Intended to run for the app lifetime.
pub async fn watch_daemon(mut notify: impl FnMut(DaemonSignal)) {
    let mut attempt: u32 = 0;
    loop {
        if let Ok(client) = BlueRouteClient::connect().await
            && let Ok(mut events) = client.events().await
        {
            attempt = 0;
            notify(DaemonSignal::Connected);
            while events.next_event().await.is_ok() {
                notify(DaemonSignal::Changed);
            }
        }
        notify(DaemonSignal::Disconnected);
        Timer::after(reconnect_delay(attempt)).await;
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

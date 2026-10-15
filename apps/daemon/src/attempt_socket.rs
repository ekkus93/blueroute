//! Owns and cancels real TCP sockets for *pending* control-handshake retries.
//!
//! This is a transport teardown primitive, NOT an authenticated session
//! manager. No socket is promoted to an authorized peer, and the daemon does
//! not yet run a TLS handshake here. A future TLS 1.3 runtime must enforce
//! its own connect/handshake deadlines and cryptographic verification.

use std::collections::BTreeMap;
use std::net::{Shutdown, TcpStream};

use blueroute_protocol::session::{PeerSessionKey, ReconnectScheduleError, ReconnectScheduler};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchdogResult {
    NotExpired,
    Cancelled { next_retry_ms: Option<u64> },
}

/// A single owner for in-flight handshake sockets and retry generations.
/// Socket shutdown/drop always precedes advancing the retry scheduler.
#[derive(Debug, Default)]
pub struct PendingHandshakeSockets {
    scheduler: ReconnectScheduler,
    sockets: BTreeMap<PeerSessionKey, ([u8; 16], TcpStream)>,
}

impl PendingHandshakeSockets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_len(&self) -> usize {
        self.sockets.len()
    }

    pub fn schedule_loss(
        &mut self,
        key: PeerSessionKey,
        generation: [u8; 16],
        now_ms: u64,
    ) -> Result<bool, ReconnectScheduleError> {
        self.scheduler.schedule_loss(key, generation, now_ms)
    }

    /// Takes ownership of a connected socket only when its retry is due.
    /// Rejected sockets are dropped; duplicate or stale generations cannot
    /// replace a live attempt. This never grants peer authorization.
    pub fn begin(
        &mut self,
        key: PeerSessionKey,
        generation: [u8; 16],
        now_ms: u64,
        socket: TcpStream,
    ) -> Result<bool, ReconnectScheduleError> {
        if self.sockets.contains_key(&key) || !self.scheduler.take_due(key, generation, now_ms)? {
            return Ok(false);
        }
        self.sockets.insert(key, (generation, socket));
        Ok(true)
    }

    /// Cancel exactly one expired handshake and close its actual socket.
    /// No retry can be scheduled until shutdown and drop have completed.
    pub fn watchdog(
        &mut self,
        key: PeerSessionKey,
        generation: [u8; 16],
        now_ms: u64,
    ) -> Result<WatchdogResult, ReconnectScheduleError> {
        if !self
            .scheduler
            .take_expired_attempt(key, generation, now_ms)?
        {
            return Ok(WatchdogResult::NotExpired);
        }
        // Only begin() can start an attempt; the generation must still match.
        // Missing socket is treated as failed, never as authenticated.
        self.close_if_current(key, generation);
        let next_retry_ms = self.scheduler.attempt_failed(key, generation, now_ms)?;
        Ok(WatchdogResult::Cancelled { next_retry_ms })
    }

    /// An ordinary connect/TLS/Hello failure follows the same teardown rule.
    /// A stale completion cannot close a newer generation.
    pub fn failed(
        &mut self,
        key: PeerSessionKey,
        generation: [u8; 16],
        now_ms: u64,
    ) -> Result<Option<u64>, ReconnectScheduleError> {
        if !self.close_if_current(key, generation) {
            return Ok(None);
        }
        // The socket is already shut down. If a regressed timestamp makes
        // scheduler bookkeeping fail, retire the generation rather than leave
        // an in-flight slot with no socket that could block future recovery.
        // Never schedule a retry on a clock error.
        match self.scheduler.attempt_failed(key, generation, now_ms) {
            Ok(next_retry_ms) => Ok(next_retry_ms),
            Err(error) => {
                self.scheduler.cancel_if_current(key, generation);
                Err(error)
            }
        }
    }

    /// Explicit shutdown/revocation: close the owned socket before removing
    /// the generation, without creating another retry. A pending backoff has
    /// no socket yet but MUST still be cancellable after peer revocation.
    pub fn cancel(&mut self, key: PeerSessionKey, generation: [u8; 16]) -> bool {
        self.close_if_current(key, generation);
        self.scheduler.cancel_if_current(key, generation)
    }

    fn close_if_current(&mut self, key: PeerSessionKey, generation: [u8; 16]) -> bool {
        if self
            .sockets
            .get(&key)
            .is_none_or(|(current, _)| *current != generation)
        {
            return false;
        }
        if let Some((_, socket)) = self.sockets.remove(&key) {
            // Even if the OS reports an already-closed socket, dropping our
            // only owned handle is mandatory before the next attempt.
            let _ = socket.shutdown(Shutdown::Both);
            drop(socket);
        }
        true
    }
}

impl Drop for PendingHandshakeSockets {
    fn drop(&mut self) {
        for (_, (_, socket)) in std::mem::take(&mut self.sockets) {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    use blueroute_core::{NetworkId, NodeId};

    fn key(id: u8) -> PeerSessionKey {
        PeerSessionKey {
            network: NetworkId::from_bytes([1; 16]),
            peer: NodeId::from_bytes([id; 16]),
        }
    }

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        (client, server)
    }

    #[test]
    fn watchdog_closes_real_socket_before_retry_is_eligible() {
        let mut owner = PendingHandshakeSockets::new();
        let peer = key(2);
        let generation = [3; 16];
        assert!(owner.schedule_loss(peer, generation, 0).unwrap());
        let (socket, mut remote) = pair();
        assert!(owner.begin(peer, generation, 250, socket).unwrap());
        assert_eq!(owner.pending_len(), 1);
        assert_eq!(
            owner.watchdog(peer, generation, 20_249).unwrap(),
            WatchdogResult::NotExpired
        );
        assert_eq!(
            owner.watchdog(peer, [4; 16], 20_250).unwrap(),
            WatchdogResult::NotExpired
        );
        assert_eq!(
            owner.watchdog(peer, generation, 20_250).unwrap(),
            WatchdogResult::Cancelled {
                next_retry_ms: Some(20_750)
            }
        );
        assert_eq!(owner.pending_len(), 0);
        let mut byte = [0];
        assert_eq!(remote.read(&mut byte).unwrap(), 0);
        assert_eq!(
            owner.watchdog(peer, generation, 20_251).unwrap(),
            WatchdogResult::NotExpired
        );
        let (early, _) = pair();
        assert!(!owner.begin(peer, generation, 20_749, early).unwrap());
        let (next, _) = pair();
        assert!(owner.begin(peer, generation, 20_750, next).unwrap());
        assert_eq!(owner.pending_len(), 1);
    }

    #[test]
    fn ordinary_handshake_failure_closes_socket_before_retry_and_ignores_stale_result() {
        let mut owner = PendingHandshakeSockets::new();
        let peer = key(7);
        let generation = [7; 16];
        assert!(owner.schedule_loss(peer, generation, 0).unwrap());
        let (socket, mut remote) = pair();
        assert!(owner.begin(peer, generation, 250, socket).unwrap());

        // An out-of-generation callback must not close the live attempt.
        assert_eq!(owner.failed(peer, [8; 16], 300).unwrap(), None);
        assert_eq!(owner.pending_len(), 1);
        remote.write_all(b"still open").unwrap();

        // Failure must close the owned socket before making retry eligible.
        assert_eq!(owner.failed(peer, generation, 300).unwrap(), Some(800));
        assert_eq!(owner.pending_len(), 0);
        assert_eq!(remote.read(&mut [0]).unwrap(), 0);
        assert_eq!(owner.failed(peer, generation, 301).unwrap(), None);

        let (early, _) = pair();
        assert!(!owner.begin(peer, generation, 799, early).unwrap());
        let (retry, _) = pair();
        assert!(owner.begin(peer, generation, 800, retry).unwrap());
    }

    #[test]
    fn revocation_cancels_queued_and_backoff_retries_without_open_sockets() {
        let mut owner = PendingHandshakeSockets::new();
        let peer = key(8);
        let old = [8; 16];
        assert!(owner.schedule_loss(peer, old, 0).unwrap());
        // No TCP socket exists until the due time. Revocation must still
        // erase the pending retry, or a revoked peer could be dialed later.
        assert_eq!(owner.pending_len(), 0);
        assert!(owner.cancel(peer, old));
        assert!(!owner.cancel(peer, old));
        let (unused, _) = pair();
        assert!(!owner.begin(peer, old, 250, unused).unwrap());

        // The same rule applies after a failed handshake has closed its
        // socket but left a future retry queued in the scheduler.
        let fresh = [9; 16];
        assert!(owner.schedule_loss(peer, fresh, 300).unwrap());
        let (socket, mut remote) = pair();
        assert!(owner.begin(peer, fresh, 550, socket).unwrap());
        assert_eq!(owner.failed(peer, fresh, 600).unwrap(), Some(1_100));
        assert_eq!(remote.read(&mut [0]).unwrap(), 0);
        assert_eq!(owner.pending_len(), 0);
        assert!(!owner.cancel(peer, old));
        assert!(owner.cancel(peer, fresh));
        let (unused, _) = pair();
        assert!(!owner.begin(peer, fresh, 1_100, unused).unwrap());
        assert!(owner.schedule_loss(peer, [10; 16], 1_200).unwrap());
    }

    #[test]
    fn stale_cancel_does_not_close_another_peer_and_drop_closes_sockets() {
        let mut owner = PendingHandshakeSockets::new();
        for id in [2_u8, 3] {
            assert!(owner.schedule_loss(key(id), [id; 16], 0).unwrap());
        }
        let (first, mut first_remote) = pair();
        let (second, mut second_remote) = pair();
        assert!(owner.begin(key(2), [2; 16], 250, first).unwrap());
        assert!(owner.begin(key(3), [3; 16], 250, second).unwrap());
        assert!(!owner.cancel(key(2), [9; 16]));
        assert_eq!(owner.pending_len(), 2);
        assert!(owner.cancel(key(2), [2; 16]));
        assert_eq!(first_remote.read(&mut [0]).unwrap(), 0);
        assert_eq!(owner.pending_len(), 1);
        second_remote.write_all(b"still open").unwrap();
        drop(owner);
        assert_eq!(second_remote.read(&mut [0]).unwrap(), 0);
    }
    #[test]
    fn regressed_failure_clock_closes_socket_and_retires_in_flight_generation() {
        let mut owner = PendingHandshakeSockets::new();
        let peer = key(9);
        let old = [9; 16];
        assert!(owner.schedule_loss(peer, old, 0).unwrap());
        let (socket, mut remote) = pair();
        assert!(owner.begin(peer, old, 250, socket).unwrap());

        // A regressed callback cannot leave a socketless in-flight retry slot.
        assert_eq!(
            owner.failed(peer, old, 249),
            Err(ReconnectScheduleError::ClockRegression)
        );
        assert_eq!(owner.pending_len(), 0);
        assert_eq!(remote.read(&mut [0]).unwrap(), 0);
        assert_eq!(
            owner.watchdog(peer, old, 20_250).unwrap(),
            WatchdogResult::NotExpired
        );

        // A fresh loss generation can be scheduled explicitly, but the stale
        // callback must never cancel or close its replacement socket.
        let fresh = [10; 16];
        assert!(owner.schedule_loss(peer, fresh, 300).unwrap());
        let (replacement, mut replacement_remote) = pair();
        assert!(owner.begin(peer, fresh, 550, replacement).unwrap());
        assert_eq!(owner.failed(peer, old, 551).unwrap(), None);
        assert!(!owner.cancel(peer, old));
        assert_eq!(owner.pending_len(), 1);
        replacement_remote.write_all(b"fresh attempt").unwrap();
        assert!(owner.cancel(peer, fresh));
        assert_eq!(replacement_remote.read(&mut [0]).unwrap(), 0);
    }
}

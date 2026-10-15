//! Owns and cancels real TCP sockets for *pending* control-handshake retries.
//!
//! This is a transport teardown primitive, NOT an authenticated session
//! manager. No socket is promoted to an authorized peer, and the daemon does
//! not yet run a TLS handshake here. A future TLS 1.3 runtime must enforce
//! its own connect/handshake deadlines and cryptographic verification.

use std::collections::BTreeMap;
use std::net::{Shutdown, TcpStream};

use blueroute_protocol::session::{
    PeerSessionKey, ReconnectScheduleError, ReconnectScheduler,
};

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
        if self.sockets.contains_key(&key)
            || !self.scheduler.take_due(key, generation, now_ms)?
        {
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
        if !self.scheduler.take_expired_attempt(key, generation, now_ms)? {
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
        self.scheduler.attempt_failed(key, generation, now_ms)
    }

    /// Explicit shutdown/revocation: close the owned socket before removing
    /// the generation, without creating another retry.
    pub fn cancel(&mut self, key: PeerSessionKey, generation: [u8; 16]) -> bool {
        if !self.close_if_current(key, generation) {
            return false;
        }
        self.scheduler.cancel_if_current(key, generation)
    }

    fn close_if_current(&mut self, key: PeerSessionKey, generation: [u8; 16]) -> bool {
        if !self
            .sockets
            .get(&key)
            .is_some_and(|(current, _)| *current == generation)
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
}

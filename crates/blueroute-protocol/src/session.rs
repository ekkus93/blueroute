//! Control-session lifecycle (P7-004).
//!
//! A pure, clock-injected state machine. It decides when to connect, keep alive, declare a
//! session lost, back off and reconnect, and which of two simultaneous sessions to keep. It
//! performs no I/O: the daemon executes the returned [`Action`]s, which keeps the lifecycle
//! deterministic to test and guarantees at most one session record per logical peer.

use std::collections::BTreeMap;
use std::time::Duration;

use blueroute_core::NodeId;

/// Identifies one transport connection (not a peer).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionConfig {
    pub keepalive_interval: Duration,
    /// Silence longer than this declares the session lost.
    pub idle_timeout: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            keepalive_interval: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(30),
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// Side effects the daemon must perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Open a control connection to the peer (data plane is available).
    Connect {
        peer: NodeId,
    },
    SendKeepalive {
        peer: NodeId,
        session: SessionId,
    },
    /// Close and release the transport connection and any task bound to it.
    Close {
        session: SessionId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PeerState {
    /// Data plane up; waiting to (re)connect. `retry_at` is when a connect may be attempted.
    Idle {
        retry_at: Duration,
    },
    Connecting,
    Established {
        session: SessionId,
        initiated_by_local: bool,
        last_heard: Duration,
        last_sent: Duration,
    },
}

#[derive(Clone, Copy, Debug)]
struct Peer {
    state: PeerState,
    failures: u32,
}

/// Verdict for a newly established session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Established {
    /// Whether the new session is the one kept for the peer.
    pub keep: bool,
    pub actions: Vec<Action>,
}

#[derive(Debug)]
pub struct SessionManager {
    local: NodeId,
    config: SessionConfig,
    peers: BTreeMap<NodeId, Peer>,
}

impl SessionManager {
    pub fn new(local: NodeId, config: SessionConfig) -> Self {
        Self {
            local,
            config,
            peers: BTreeMap::new(),
        }
    }

    /// Number of peers with any lifecycle record (bounded by the number of known peers).
    pub fn tracked_peers(&self) -> usize {
        self.peers.len()
    }

    pub fn established_session(&self, peer: NodeId) -> Option<SessionId> {
        match self.peers.get(&peer)?.state {
            PeerState::Established { session, .. } => Some(session),
            _ => None,
        }
    }

    fn backoff(&self, failures: u32) -> Duration {
        let factor = 1u32 << failures.saturating_sub(1).min(16);
        self.config
            .backoff_base
            .saturating_mul(factor)
            .min(self.config.backoff_max)
    }

    /// Only the lower `NodeId` dials, so two nodes rarely connect to each other at once.
    fn local_dials(&self, peer: NodeId) -> bool {
        self.local < peer
    }

    /// The PAN link to `peer` is usable. Control sessions start only after this.
    pub fn data_plane_available(&mut self, peer: NodeId, now: Duration) -> Vec<Action> {
        if self.peers.contains_key(&peer) {
            return Vec::new();
        }
        self.peers.insert(
            peer,
            Peer {
                state: PeerState::Idle { retry_at: now },
                failures: 0,
            },
        );
        self.try_connect(peer, now)
    }

    fn try_connect(&mut self, peer: NodeId, now: Duration) -> Vec<Action> {
        let dials = self.local_dials(peer);
        let Some(entry) = self.peers.get_mut(&peer) else {
            return Vec::new();
        };
        match entry.state {
            PeerState::Idle { retry_at } if dials && now >= retry_at => {
                entry.state = PeerState::Connecting;
                vec![Action::Connect { peer }]
            }
            _ => Vec::new(),
        }
    }

    /// The PAN link to `peer` is gone. Drops the record; nothing reconnects until the data
    /// plane returns.
    pub fn data_plane_lost(&mut self, peer: NodeId) -> Vec<Action> {
        match self.peers.remove(&peer).map(|p| p.state) {
            Some(PeerState::Established { session, .. }) => vec![Action::Close { session }],
            _ => Vec::new(),
        }
    }

    /// A connection attempt failed before a session formed.
    pub fn connect_failed(&mut self, peer: NodeId, now: Duration) {
        let Some(failures) = self.peers.get(&peer).map(|p| p.failures + 1) else {
            return;
        };
        let retry_at = now + self.backoff(failures);
        if let Some(entry) = self.peers.get_mut(&peer) {
            entry.failures = failures;
            entry.state = PeerState::Idle { retry_at };
        }
    }

    /// A session finished its handshake and hello. Keeps one session per peer. When two exist
    /// (both sides dialed), both ends keep the session initiated by the lower `NodeId`.
    pub fn session_established(
        &mut self,
        peer: NodeId,
        session: SessionId,
        initiated_by_local: bool,
        now: Duration,
    ) -> Established {
        let new_wins_tiebreak = initiated_by_local == (self.local < peer);
        let entry = self.peers.entry(peer).or_insert(Peer {
            state: PeerState::Connecting,
            failures: 0,
        });
        let mut actions = Vec::new();
        let keep = match entry.state {
            PeerState::Established {
                session: old,
                initiated_by_local: old_local,
                ..
            } if old != session => {
                let old_wins = old_local == (self.local < peer);
                if old_wins && !new_wins_tiebreak {
                    actions.push(Action::Close { session });
                    false
                } else {
                    actions.push(Action::Close { session: old });
                    true
                }
            }
            _ => true,
        };
        if keep {
            entry.failures = 0;
            entry.state = PeerState::Established {
                session,
                initiated_by_local,
                last_heard: now,
                last_sent: now,
            };
        }
        Established { keep, actions }
    }

    /// Any authenticated inbound message counts as liveness. Ignored for stale sessions.
    pub fn heard_from(&mut self, peer: NodeId, session: SessionId, now: Duration) {
        if let Some(Peer {
            state:
                PeerState::Established {
                    session: current,
                    last_heard,
                    ..
                },
            ..
        }) = self.peers.get_mut(&peer)
            && *current == session
        {
            *last_heard = now;
        }
    }

    /// The transport reported the session closed or errored.
    pub fn session_closed(&mut self, peer: NodeId, session: SessionId, now: Duration) {
        let retry_at = now + self.backoff(1);
        if let Some(entry) = self.peers.get_mut(&peer)
            && matches!(entry.state, PeerState::Established { session: s, .. } if s == session)
        {
            entry.failures = 1;
            entry.state = PeerState::Idle { retry_at };
        }
    }

    /// Advance timers: keepalives, loss detection, and due reconnects.
    pub fn tick(&mut self, now: Duration) -> Vec<Action> {
        let mut actions = Vec::new();
        let peers: Vec<NodeId> = self.peers.keys().copied().collect();
        for peer in peers {
            let config = self.config;
            let retry = self.backoff(1);
            let Some(entry) = self.peers.get_mut(&peer) else {
                continue;
            };
            if let PeerState::Established {
                session,
                last_heard,
                last_sent,
                ..
            } = &mut entry.state
            {
                if now.saturating_sub(*last_heard) >= config.idle_timeout {
                    actions.push(Action::Close { session: *session });
                    entry.failures = 1;
                    entry.state = PeerState::Idle {
                        retry_at: now + retry,
                    };
                } else if now.saturating_sub(*last_sent) >= config.keepalive_interval {
                    *last_sent = now;
                    actions.push(Action::SendKeepalive {
                        peer,
                        session: *session,
                    });
                }
            }
            actions.extend(self.try_connect(peer, now));
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    fn manager(local: u8) -> SessionManager {
        SessionManager::new(n(local), SessionConfig::default())
    }

    #[test]
    fn session_starts_only_after_data_plane_and_only_the_lower_id_dials() {
        let mut low = manager(1);
        let mut high = manager(2);
        assert!(low.tick(s(0)).is_empty(), "no peer, no connect");
        assert_eq!(
            low.data_plane_available(n(2), s(0)),
            vec![Action::Connect { peer: n(2) }]
        );
        assert!(high.data_plane_available(n(1), s(0)).is_empty());
    }

    #[test]
    fn silence_is_detected_then_reconnect_backs_off() {
        let mut m = manager(1);
        m.data_plane_available(n(2), s(0));
        m.session_established(n(2), SessionId(1), true, s(0));
        assert_eq!(
            m.tick(s(10)),
            vec![Action::SendKeepalive {
                peer: n(2),
                session: SessionId(1)
            }]
        );
        assert_eq!(
            m.tick(s(30)),
            vec![Action::Close {
                session: SessionId(1)
            }]
        );
        assert!(m.established_session(n(2)).is_none());
        assert!(m.tick(s(30)).is_empty(), "backoff delays the redial");
        assert_eq!(m.tick(s(31)), vec![Action::Connect { peer: n(2) }]);
    }

    #[test]
    fn liveness_from_the_peer_defers_loss() {
        let mut m = manager(1);
        m.data_plane_available(n(2), s(0));
        m.session_established(n(2), SessionId(1), true, s(0));
        m.heard_from(n(2), SessionId(1), s(25));
        assert!(!m.tick(s(40)).contains(&Action::Close {
            session: SessionId(1)
        }));
        m.heard_from(n(2), SessionId(99), s(41)); // stale session id is ignored
        assert!(m.tick(s(56)).contains(&Action::Close {
            session: SessionId(1)
        }));
    }

    #[test]
    fn failed_connects_back_off_exponentially_up_to_the_cap() {
        let mut m = manager(1);
        m.data_plane_available(n(2), s(0));
        let mut now = s(0);
        let mut waits = Vec::new();
        for _ in 0..8 {
            m.connect_failed(n(2), now);
            let mut t = now;
            while m.tick(t).is_empty() {
                t += s(1);
            }
            waits.push((t - now).as_secs());
            now = t;
        }
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn simultaneous_sessions_collapse_to_the_lower_ids_connection() {
        // Local 1 vs peer 2: the session initiated by local (lower id) wins on both ends.
        let mut low = manager(1);
        low.session_established(n(2), SessionId(1), false, s(0));
        let r = low.session_established(n(2), SessionId(2), true, s(1));
        assert!(r.keep);
        assert_eq!(
            r.actions,
            vec![Action::Close {
                session: SessionId(1)
            }]
        );
        assert_eq!(low.established_session(n(2)), Some(SessionId(2)));

        // Same two sessions seen from the higher id: the inbound (peer-initiated) one wins.
        let mut high = manager(2);
        high.session_established(n(1), SessionId(10), true, s(0));
        let r = high.session_established(n(1), SessionId(11), false, s(1));
        assert!(r.keep);
        assert_eq!(high.established_session(n(1)), Some(SessionId(11)));
        // Arrival order must not matter.
        let mut high2 = manager(2);
        high2.session_established(n(1), SessionId(11), false, s(0));
        let r = high2.session_established(n(1), SessionId(10), true, s(1));
        assert!(!r.keep);
        assert_eq!(
            r.actions,
            vec![Action::Close {
                session: SessionId(10)
            }]
        );
        assert_eq!(high2.established_session(n(1)), Some(SessionId(11)));
    }

    #[test]
    fn repeated_pan_reconnects_do_not_leak_sessions() {
        let mut m = manager(1);
        let mut open = std::collections::BTreeSet::new();
        for round in 0..200u64 {
            let now = s(round * 100);
            m.data_plane_available(n(2), now);
            let id = SessionId(round);
            let r = m.session_established(n(2), id, true, now);
            assert!(r.keep);
            open.insert(id);
            for action in m.data_plane_lost(n(2)) {
                if let Action::Close { session } = action {
                    open.remove(&session);
                }
            }
            assert!(open.is_empty(), "round {round} leaked a session");
            assert_eq!(m.tracked_peers(), 0);
        }
    }

    #[test]
    fn closed_session_is_released_and_redialed_after_backoff() {
        let mut m = manager(1);
        m.data_plane_available(n(2), s(0));
        m.session_established(n(2), SessionId(1), true, s(0));
        m.session_closed(n(2), SessionId(1), s(5));
        assert!(m.established_session(n(2)).is_none());
        assert_eq!(m.tick(s(6)), vec![Action::Connect { peer: n(2) }]);
        m.session_closed(n(2), SessionId(77), s(7)); // stale close does not disturb state
    }
}

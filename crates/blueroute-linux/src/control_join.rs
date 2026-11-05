//! Join exchange over an authenticated control channel (ADR 0001, P6-004 prerequisite).
//!
//! Host (network authority): after the handshake proves the peer's `NodeId`, check approval
//! (P6-003), exchange hellos, and issue a time-bounded membership credential, or deny.
//! Joiner: dial with the authority's `NodeId` pinned so the handshake itself proves who
//! answered, exchange hellos, request a credential, and verify it against the key the
//! handshake proved. No source address, name, or advertisement is consulted for identity.

use std::fmt;

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::{ControlEnvelope, ControlError, Inbound, InboundValidator};
use blueroute_protocol::crypto::{Credential, CryptoError, NodeIdentity};
use blueroute_protocol::hello::{Hello, HelloError, Negotiated, negotiate};
use blueroute_protocol::join::{DenyReason, JoinMessage};
use futures_lite::io::{AsyncRead, AsyncWrite};

use crate::control_transport::{ControlChannel, TransportError};

/// Default credential lifetime. Short so revocation takes effect without a deny-list push.
pub const CREDENTIAL_TTL_SECS: u64 = 60 * 60;
/// Tolerated clock difference between nodes when verifying credential windows.
pub const CLOCK_SKEW_SECS: u64 = 5 * 60;

#[derive(Debug)]
pub enum JoinError {
    Transport(TransportError),
    Frame(ControlError),
    Hello(HelloError),
    Credential(CryptoError),
    /// The authority refused the join.
    Denied(DenyReason),
    /// The peer sent a message that is not valid at this point of the exchange.
    Protocol(&'static str),
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Frame(e) => write!(f, "{e}"),
            Self::Hello(e) => write!(f, "{e}"),
            Self::Credential(e) => write!(f, "credential rejected: {e}"),
            Self::Denied(reason) => write!(f, "authority denied the join: {reason:?}"),
            Self::Protocol(what) => write!(f, "control protocol violation: {what}"),
        }
    }
}

impl std::error::Error for JoinError {}

impl From<TransportError> for JoinError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<ControlError> for JoinError {
    fn from(e: ControlError) -> Self {
        Self::Frame(e)
    }
}

impl From<HelloError> for JoinError {
    fn from(e: HelloError) -> Self {
        Self::Hello(e)
    }
}

/// One side's sequencing and inbound validation for the session.
struct Session<'a, S> {
    channel: &'a mut ControlChannel<S>,
    network: NetworkId,
    local: NodeId,
    next_sequence: u64,
    inbound: InboundValidator,
}

impl<'a, S: AsyncRead + AsyncWrite + Unpin> Session<'a, S> {
    fn new(channel: &'a mut ControlChannel<S>, network: NetworkId, local: NodeId) -> Self {
        let inbound = InboundValidator::new(network, channel.peer());
        Self {
            channel,
            network,
            local,
            next_sequence: 1,
            inbound,
        }
    }

    async fn send_hello(&mut self, hello: &Hello) -> Result<(), JoinError> {
        let envelope = hello.to_envelope(self.network, self.next_sequence)?;
        self.next_sequence += 1;
        Ok(self.channel.send(&envelope).await?)
    }

    async fn send_join(&mut self, message: &JoinMessage) -> Result<(), JoinError> {
        let envelope = message.to_envelope(self.network, self.local, self.next_sequence);
        self.next_sequence += 1;
        Ok(self.channel.send(&envelope).await?)
    }

    /// Receive the next message this daemon understands; unknown non-critical ones are skipped.
    async fn recv(&mut self) -> Result<ControlEnvelope, JoinError> {
        loop {
            let envelope = self.channel.recv().await?;
            if let Inbound::Handle(envelope) = self.inbound.accept(envelope)? {
                return Ok(envelope);
            }
        }
    }

    async fn recv_hello(&mut self) -> Result<Hello, JoinError> {
        Ok(Hello::from_envelope(&self.recv().await?)?)
    }

    async fn recv_join(&mut self) -> Result<JoinMessage, JoinError> {
        JoinMessage::from_envelope(&self.recv().await?)
            .ok_or(JoinError::Protocol("expected a join message"))
    }
}

pub struct HostJoinPolicy<'a> {
    pub network: NetworkId,
    /// The authority key; also the identity this host proved in the handshake.
    pub authority: &'a NodeIdentity,
    /// Whether the user approved this node for this network (P6-003).
    pub is_approved: &'a dyn Fn(NodeId) -> bool,
    /// Seconds since the Unix epoch.
    pub now: u64,
    pub credential_ttl_secs: u64,
}

#[derive(Debug)]
pub struct HostJoinOutcome {
    pub peer: NodeId,
    pub negotiated: Negotiated,
    pub credential: Option<Credential>,
}

/// Serve one join exchange on an accepted channel.
pub async fn serve_join<S: AsyncRead + AsyncWrite + Unpin>(
    channel: &mut ControlChannel<S>,
    policy: &HostJoinPolicy<'_>,
    local_hello: &Hello,
) -> Result<HostJoinOutcome, JoinError> {
    let peer = channel.peer();
    let mut session = Session::new(channel, policy.network, policy.authority.node_id());
    session.send_hello(local_hello).await?;
    let remote_hello = session.recv_hello().await?;
    let negotiated = negotiate(local_hello, &remote_hello)?;
    if session.recv_join().await? != JoinMessage::Request {
        return Err(JoinError::Protocol("expected join.request"));
    }
    if !(policy.is_approved)(peer) {
        session
            .send_join(&JoinMessage::Denied {
                reason: DenyReason::NotApproved,
            })
            .await?;
        return Ok(HostJoinOutcome {
            peer,
            negotiated,
            credential: None,
        });
    }
    let credential = policy
        .authority
        .issue_credential(
            policy.network,
            peer,
            policy.now,
            policy.now.saturating_add(policy.credential_ttl_secs),
        )
        .map_err(JoinError::Credential)?;
    session
        .send_join(&JoinMessage::Credential {
            credential: credential.clone(),
        })
        .await?;
    Ok(HostJoinOutcome {
        peer,
        negotiated,
        credential: Some(credential),
    })
}

#[derive(Debug)]
pub struct JoinerOutcome {
    pub negotiated: Negotiated,
    pub credential: Credential,
}

/// Request membership from the authority on a channel dialed with the authority pinned.
pub async fn request_join<S: AsyncRead + AsyncWrite + Unpin>(
    channel: &mut ControlChannel<S>,
    network: NetworkId,
    local: &NodeIdentity,
    local_hello: &Hello,
    now: u64,
) -> Result<JoinerOutcome, JoinError> {
    let authority_key = channel.peer_public_key();
    let mut session = Session::new(channel, network, local.node_id());
    let remote_hello = session.recv_hello().await?;
    session.send_hello(local_hello).await?;
    let negotiated = negotiate(local_hello, &remote_hello)?;
    session.send_join(&JoinMessage::Request).await?;
    match session.recv_join().await? {
        JoinMessage::Credential { credential } => {
            credential
                .verify(
                    &authority_key,
                    network,
                    local.node_id(),
                    now,
                    CLOCK_SKEW_SECS,
                )
                .map_err(JoinError::Credential)?;
            Ok(JoinerOutcome {
                negotiated,
                credential,
            })
        }
        JoinMessage::Denied { reason } => Err(JoinError::Denied(reason)),
        JoinMessage::Request => Err(JoinError::Protocol(
            "unexpected join.request from authority",
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use async_io::Async;
    use blueroute_core::NodeCapabilities;
    use blueroute_protocol::control::ControlVersion;
    use blueroute_protocol::crypto::SEED_BYTES;
    use futures_lite::future::{block_on, zip};

    use super::*;

    fn identity(b: u8) -> NodeIdentity {
        NodeIdentity::from_seed(&[b; SEED_BYTES])
    }

    fn network() -> NetworkId {
        NetworkId::from_bytes([6; 16])
    }

    fn hello(identity: &NodeIdentity) -> Hello {
        Hello {
            software: "blueroute-test".into(),
            protocol: ControlVersion::CURRENT,
            node: identity.node_id(),
            features: Default::default(),
            capabilities: NodeCapabilities::default(),
            segments: Vec::new(),
        }
    }

    async fn channels(
        host: &NodeIdentity,
        joiner: &NodeIdentity,
        pinned_authority: Option<NodeId>,
    ) -> (
        Result<ControlChannel<Async<TcpStream>>, TransportError>,
        Result<ControlChannel<Async<TcpStream>>, TransportError>,
    ) {
        let listener = Async::<TcpListener>::bind(([127, 0, 0, 1], 0)).unwrap();
        let addr = listener.get_ref().local_addr().unwrap();
        let (client, accepted) = zip(Async::<TcpStream>::connect(addr), listener.accept()).await;
        let (joiner_channel, host_channel) = zip(
            ControlChannel::connect(client.unwrap(), joiner, pinned_authority),
            ControlChannel::accept(accepted.unwrap().0, host),
        )
        .await;
        (joiner_channel, host_channel)
    }

    fn run_join(
        host: &NodeIdentity,
        joiner: &NodeIdentity,
        pinned: Option<NodeId>,
        approved: bool,
        host_now: u64,
        joiner_now: u64,
    ) -> (Result<JoinerOutcome, JoinError>, HostJoinOutcome) {
        block_on(async {
            let (jc, hc) = channels(host, joiner, pinned).await;
            let (mut jc, mut hc) = (jc.unwrap(), hc.unwrap());
            let is_approved = move |_node: NodeId| approved;
            let policy = HostJoinPolicy {
                network: network(),
                authority: host,
                is_approved: &is_approved,
                now: host_now,
                credential_ttl_secs: CREDENTIAL_TTL_SECS,
            };
            let (host_hello, joiner_hello) = (hello(host), hello(joiner));
            let (joined, served) = zip(
                request_join(&mut jc, network(), joiner, &joiner_hello, joiner_now),
                serve_join(&mut hc, &policy, &host_hello),
            )
            .await;
            (joined, served.unwrap())
        })
    }

    #[test]
    fn approved_node_receives_a_credential_that_verifies_against_the_pinned_authority() {
        let (host, joiner) = (identity(1), identity(2));
        let (joined, served) = run_join(&host, &joiner, Some(host.node_id()), true, 1_000, 1_000);
        let joined = joined.unwrap();
        assert_eq!(served.peer, joiner.node_id());
        assert_eq!(joined.credential, served.credential.unwrap());
        joined
            .credential
            .verify(&host.public_key(), network(), joiner.node_id(), 1_100, 0)
            .unwrap();
        assert_eq!(joined.negotiated.peer, host.node_id());
    }

    #[test]
    fn unapproved_node_is_denied_and_gets_no_credential() {
        let (host, joiner) = (identity(1), identity(2));
        let (joined, served) = run_join(&host, &joiner, Some(host.node_id()), false, 1_000, 1_000);
        assert!(matches!(
            joined.unwrap_err(),
            JoinError::Denied(DenyReason::NotApproved)
        ));
        assert!(served.credential.is_none());
    }

    #[test]
    fn rogue_authority_fails_the_pin_before_any_join_message() {
        block_on(async {
            let (rogue, joiner, real) = (identity(9), identity(2), identity(1));
            let (jc, _hc) = channels(&rogue, &joiner, Some(real.node_id())).await;
            assert!(matches!(
                jc.unwrap_err(),
                TransportError::Handshake(
                    blueroute_protocol::handshake::HandshakeError::UnexpectedPeer { .. }
                )
            ));
        });
    }

    #[test]
    fn credential_outside_the_clock_window_is_rejected_by_the_joiner() {
        let (host, joiner) = (identity(1), identity(2));
        // Joiner clock is a day ahead of the host: the one-hour credential has expired.
        let (joined, _served) = run_join(
            &host,
            &joiner,
            Some(host.node_id()),
            true,
            1_000,
            1_000 + 86_400,
        );
        assert!(matches!(
            joined.unwrap_err(),
            JoinError::Credential(CryptoError::CredentialExpired)
        ));
    }

    #[test]
    fn credential_is_bound_to_the_proven_node_not_to_a_claimed_one() {
        let (host, joiner) = (identity(1), identity(2));
        let (joined, _) = run_join(&host, &joiner, None, true, 5, 5);
        let credential = joined.unwrap().credential;
        assert_eq!(credential.node, joiner.node_id());
        assert!(
            credential
                .verify(&host.public_key(), network(), identity(3).node_id(), 5, 0)
                .is_err()
        );
    }
}

//! Authenticated control channel over any async byte stream (ADR 0001, P7-001 follow-up).
//!
//! Wire format on the stream: `u16_be(length) || bytes`. The first three records are the
//! Noise XX handshake messages; afterwards each record is the AEAD ciphertext of one P7-002
//! frame (`u32 length || JSON envelope`). Lengths are bounded by the `u16` prefix, and the
//! decoded envelope is bounded again by `MAX_CONTROL_FRAME_BYTES`. The handshake has a
//! deadline so a silent peer cannot hold a session slot.

use std::fmt;
use std::io;
use std::time::Duration;

use async_io::Timer;
use blueroute_core::NodeId;
use blueroute_protocol::control::{ControlEnvelope, ControlError, decode_frame, encode_frame};
use blueroute_protocol::crypto::NodeIdentity;
use blueroute_protocol::handshake::{Handshake, HandshakeError, Role, SecureSession};
use futures_lite::future;
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    Handshake(HandshakeError),
    Frame(ControlError),
    Timeout,
    /// The peer closed the stream cleanly between records.
    Closed,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "control transport I/O error: {e}"),
            Self::Handshake(e) => write!(f, "control handshake failed: {e}"),
            Self::Frame(e) => write!(f, "control frame rejected: {e}"),
            Self::Timeout => f.write_str("control handshake timed out"),
            Self::Closed => f.write_str("control connection closed"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<HandshakeError> for TransportError {
    fn from(e: HandshakeError) -> Self {
        Self::Handshake(e)
    }
}

impl From<ControlError> for TransportError {
    fn from(e: ControlError) -> Self {
        Self::Frame(e)
    }
}

async fn write_record<S: AsyncWrite + Unpin>(
    stream: &mut S,
    bytes: &[u8],
) -> Result<(), TransportError> {
    let len = u16::try_from(bytes.len())
        .map_err(|_| TransportError::Handshake(HandshakeError::MessageTooLarge))?;
    let mut record = Vec::with_capacity(2 + bytes.len());
    record.extend_from_slice(&len.to_be_bytes());
    record.extend_from_slice(bytes);
    stream.write_all(&record).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_record<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Vec<u8>, TransportError> {
    let mut header = [0_u8; 2];
    match stream.read_exact(&mut header).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(TransportError::Closed),
        Err(e) => return Err(e.into()),
    }
    let mut body = vec![0_u8; usize::from(u16::from_be_bytes(header))];
    stream.read_exact(&mut body).await?;
    Ok(body)
}

async fn with_deadline<T>(
    deadline: Duration,
    work: impl Future<Output = Result<T, TransportError>>,
) -> Result<T, TransportError> {
    future::or(work, async {
        Timer::after(deadline).await;
        Err(TransportError::Timeout)
    })
    .await
}

/// An established, authenticated, encrypted control connection.
pub struct ControlChannel<S> {
    stream: S,
    session: SecureSession,
}

impl<S> fmt::Debug for ControlChannel<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlChannel")
            .field("peer", &self.session.peer())
            .finish_non_exhaustive()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> ControlChannel<S> {
    /// Dial side. `expected_peer` pins who must answer.
    pub async fn connect(
        stream: S,
        identity: &NodeIdentity,
        expected_peer: Option<NodeId>,
    ) -> Result<Self, TransportError> {
        Self::connect_with_deadline(stream, identity, expected_peer, HANDSHAKE_TIMEOUT).await
    }

    pub async fn connect_with_deadline(
        mut stream: S,
        identity: &NodeIdentity,
        expected_peer: Option<NodeId>,
        deadline: Duration,
    ) -> Result<Self, TransportError> {
        let mut hs = Handshake::new(Role::Initiator, identity, expected_peer)?;
        let session = with_deadline(deadline, async {
            write_record(&mut stream, &hs.write_message()?).await?;
            hs.read_message(&read_record(&mut stream).await?)?;
            write_record(&mut stream, &hs.write_message()?).await?;
            Ok(hs.finish()?)
        })
        .await?;
        Ok(Self { stream, session })
    }

    /// Listen side.
    pub async fn accept(stream: S, identity: &NodeIdentity) -> Result<Self, TransportError> {
        Self::accept_with_deadline(stream, identity, HANDSHAKE_TIMEOUT).await
    }

    pub async fn accept_with_deadline(
        mut stream: S,
        identity: &NodeIdentity,
        deadline: Duration,
    ) -> Result<Self, TransportError> {
        let mut hs = Handshake::new(Role::Responder, identity, None)?;
        let session = with_deadline(deadline, async {
            hs.read_message(&read_record(&mut stream).await?)?;
            write_record(&mut stream, &hs.write_message()?).await?;
            hs.read_message(&read_record(&mut stream).await?)?;
            Ok(hs.finish()?)
        })
        .await?;
        Ok(Self { stream, session })
    }

    /// Proven identity of the remote node; pass it to `InboundValidator::new`.
    pub fn peer(&self) -> NodeId {
        self.session.peer()
    }

    /// Ed25519 identity key proven by the handshake (the pinned authority key for a joiner).
    pub fn peer_public_key(&self) -> [u8; 32] {
        self.session.peer_public_key()
    }

    pub async fn send(&mut self, envelope: &ControlEnvelope) -> Result<(), TransportError> {
        let frame = encode_frame(envelope)?;
        let ciphertext = self.session.encrypt(&frame)?;
        write_record(&mut self.stream, &ciphertext).await
    }

    pub async fn recv(&mut self) -> Result<ControlEnvelope, TransportError> {
        let ciphertext = read_record(&mut self.stream).await?;
        let frame = self.session.decrypt(&ciphertext)?;
        Ok(decode_frame(&frame)?)
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use async_io::Async;
    use blueroute_core::NetworkId;
    use blueroute_protocol::control::{ControlVersion, Inbound, InboundValidator, MessageType};
    use blueroute_protocol::crypto::SEED_BYTES;
    use futures_lite::future::{block_on, zip};

    use super::*;

    fn identity(b: u8) -> NodeIdentity {
        NodeIdentity::from_seed(&[b; SEED_BYTES])
    }

    fn envelope(sender: NodeId, sequence: u64) -> ControlEnvelope {
        ControlEnvelope {
            version: ControlVersion::CURRENT,
            network: NetworkId::from_bytes([4; 16]),
            sender,
            sequence,
            message_type: MessageType::new("hello").unwrap(),
            critical: false,
            payload: serde_json::json!({"k": sequence}),
        }
    }

    async fn pair() -> (Async<TcpStream>, Async<TcpStream>) {
        let listener = Async::<TcpListener>::bind(([127, 0, 0, 1], 0)).unwrap();
        let addr = listener.get_ref().local_addr().unwrap();
        let (client, accepted) = zip(Async::<TcpStream>::connect(addr), listener.accept()).await;
        (client.unwrap(), accepted.unwrap().0)
    }

    #[test]
    fn handshake_and_encrypted_envelopes_over_a_real_tcp_loopback() {
        block_on(async {
            let (a, b) = (identity(1), identity(2));
            let (c, s) = pair().await;
            let (client, server) = zip(
                ControlChannel::connect(c, &a, Some(b.node_id())),
                ControlChannel::accept(s, &b),
            )
            .await;
            let (mut client, mut server) = (client.unwrap(), server.unwrap());
            assert_eq!(client.peer(), b.node_id());
            assert_eq!(server.peer(), a.node_id());

            client.send(&envelope(a.node_id(), 1)).await.unwrap();
            client.send(&envelope(a.node_id(), 2)).await.unwrap();
            let mut validator =
                InboundValidator::new(NetworkId::from_bytes([4; 16]), server.peer());
            for expected in [1, 2] {
                let got = server.recv().await.unwrap();
                assert!(
                    matches!(validator.accept(got), Ok(Inbound::Handle(e)) if e.sequence == expected)
                );
            }
            server.send(&envelope(b.node_id(), 1)).await.unwrap();
            assert_eq!(client.recv().await.unwrap().sender, b.node_id());
        });
    }

    #[test]
    fn dialing_the_wrong_node_fails_the_handshake() {
        block_on(async {
            let (c, s) = pair().await;
            let (client, _server) = zip(
                ControlChannel::connect(c, &identity(1), Some(identity(7).node_id())),
                ControlChannel::accept(s, &identity(2)),
            )
            .await;
            assert!(matches!(
                client.unwrap_err(),
                TransportError::Handshake(HandshakeError::UnexpectedPeer { .. })
            ));
        });
    }

    #[test]
    fn silent_peer_times_out_instead_of_holding_the_slot() {
        block_on(async {
            let (_silent, s) = pair().await;
            let started = std::time::Instant::now();
            let result =
                ControlChannel::accept_with_deadline(s, &identity(2), Duration::from_millis(150))
                    .await;
            assert!(matches!(result.unwrap_err(), TransportError::Timeout));
            assert!(started.elapsed() < Duration::from_secs(5));
        });
    }

    #[test]
    fn garbage_and_early_close_are_errors_not_panics() {
        block_on(async {
            let (mut c, s) = pair().await;
            c.write_all(&[0, 40]).await.unwrap();
            c.write_all(&[0xAA; 40]).await.unwrap();
            let result = ControlChannel::accept(s, &identity(2)).await;
            assert!(matches!(result.unwrap_err(), TransportError::Handshake(_)));

            let (c, s) = pair().await;
            drop(c);
            let result = ControlChannel::accept(s, &identity(2)).await;
            assert!(matches!(result.unwrap_err(), TransportError::Closed));
        });
    }

    #[test]
    fn tampered_ciphertext_after_handshake_is_rejected() {
        block_on(async {
            let (a, b) = (identity(1), identity(2));
            let (c, s) = pair().await;
            let (client, server) = zip(
                ControlChannel::connect(c, &a, None),
                ControlChannel::accept(s, &b),
            )
            .await;
            let (mut client, mut server) = (client.unwrap(), server.unwrap());
            let frame = encode_frame(&envelope(a.node_id(), 1)).unwrap();
            let mut ciphertext = client.session.encrypt(&frame).unwrap();
            ciphertext[3] ^= 0xFF;
            write_record(&mut client.stream, &ciphertext).await.unwrap();
            assert!(matches!(
                server.recv().await,
                Err(TransportError::Handshake(_))
            ));
        });
    }
}

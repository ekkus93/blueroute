//! Deterministic hello/ack transcript validation for future mutual TLS sessions.
//!
//! A matching transcript is NOT proof of identity, possession or channel binding.
//! The caller must separately verify TLS 1.3, pinned peer keys, enrollment,
//! NetworkId/NodeId approval and fresh exporter-bound session IDs. Production
//! JoinNetwork stays disabled until those requirements are implemented.

use crate::control::{
    ControlEnvelope, ControlMessage, HelloFacts, VerifiedSessionSequence, encode_control_frame,
};
use blueroute_core::{NetworkId, NodeId};
use std::fmt;
use std::time::Instant;

use crate::control_io::{
    ControlIoError, DeadlineControlRead, DeadlineControlWrite,
    read_one_control_frame_with_deadline, write_one_control_frame_with_deadline,
};

/// The rustls TLS 1.3 exporter label for BlueRoute control sessions.
/// Both endpoints MUST export 16 bytes with this label and the canonical
/// network/node context below from the same fully verified TLS connection.
/// These helpers do NOT perform TLS or verify the peer certificate.
pub const TLS_CONTROL_EXPORTER_LABEL: &[u8] = b"EXPORTER-BlueRoute-Control-v1";
pub const TLS_CONTROL_EXPORTER_BYTES: usize = 16;

/// A direction-independent exporter context: NetworkId || smaller NodeId ||
/// larger NodeId. This prevents accidentally binding a control transcript to
/// another network or peer pair. The TLS exporter, not this context, provides
/// cryptographic channel binding and fresh per-connection entropy.
pub fn tls_control_exporter_context(
    network: NetworkId,
    first: NodeId,
    second: NodeId,
) -> Result<[u8; 48], &'static str> {
    if network == NetworkId::from_bytes([0; 16])
        || first == NodeId::from_bytes([0; 16])
        || second == NodeId::from_bytes([0; 16])
    {
        return Err("control exporter requires nonzero network and node identities");
    }
    if first == second {
        return Err("control exporter requires two distinct nodes");
    }
    let (low, high) = if first < second {
        (first, second)
    } else {
        (second, first)
    };
    let mut context = [0_u8; 48];
    context[..16].copy_from_slice(network.as_bytes());
    context[16..32].copy_from_slice(low.as_bytes());
    context[32..].copy_from_slice(high.as_bytes());
    Ok(context)
}

/// Verify that the claimed hello/ack session ID is exactly the exporter
/// output of the verified TLS connection. Never obtain `exported_session_id`
/// from a peer message, advertisement, local RNG or an unverified socket.
/// The caller MUST still enforce the approved pinned TLS peer identity.
pub fn validate_exporter_bound_hello_transcript(
    network: NetworkId,
    local: NodeId,
    remote: NodeId,
    exported_session_id: [u8; TLS_CONTROL_EXPORTER_BYTES],
    frames: &[ControlEnvelope; 4],
) -> Result<HelloFacts, &'static str> {
    let [local_hello, remote_hello, local_ack, remote_ack] = frames;
    if exported_session_id.iter().all(|byte| *byte == 0) {
        return Err("invalid zero TLS exporter session ID");
    }
    if local_hello.session_id != exported_session_id {
        return Err("control session is not bound to TLS exporter");
    }
    validate_hello_transcript(
        network,
        local,
        remote,
        local_hello,
        remote_hello,
        local_ack,
        remote_ack,
    )
}

/// Validates both directions of a two-challenge handshake. Each side sends
/// Hello(sequence=1) then HelloAck(sequence=2), echoing the other challenge.
/// The remote's claims must be identical in Hello and HelloAck.
pub fn validate_hello_transcript(
    network: NetworkId,
    local: NodeId,
    remote: NodeId,
    local_hello: &ControlEnvelope,
    remote_hello: &ControlEnvelope,
    local_ack: &ControlEnvelope,
    remote_ack: &ControlEnvelope,
) -> Result<HelloFacts, &'static str> {
    if local == remote {
        return Err("local and remote node must differ");
    }
    let frames = [local_hello, remote_hello, local_ack, remote_ack];
    for frame in frames {
        encode_control_frame(frame).map_err(|_| "malformed control frame")?;
        if frame.network != network {
            return Err("wrong network");
        }
    }
    if local_hello.sender != local
        || local_ack.sender != local
        || remote_hello.sender != remote
        || remote_ack.sender != remote
    {
        return Err("wrong sender");
    }
    let session = local_hello.session_id;
    if frames.iter().any(|frame| frame.session_id != session) {
        return Err("wrong session");
    }
    if local_hello.sequence != 1
        || remote_hello.sequence != 1
        || local_ack.sequence != 2
        || remote_ack.sequence != 2
    {
        return Err("unexpected handshake sequence");
    }
    let ControlMessage::Hello {
        challenge: local_challenge,
        facts: local_facts,
    } = &local_hello.message
    else {
        return Err("missing local hello");
    };
    let ControlMessage::Hello {
        challenge: remote_challenge,
        facts: remote_facts,
    } = &remote_hello.message
    else {
        return Err("missing remote hello");
    };
    let ControlMessage::HelloAck {
        challenge: local_echo,
        facts: local_ack_facts,
    } = &local_ack.message
    else {
        return Err("missing local acknowledgement");
    };
    let ControlMessage::HelloAck {
        challenge: remote_echo,
        facts: remote_ack_facts,
    } = &remote_ack.message
    else {
        return Err("missing remote acknowledgement");
    };
    if local_challenge.iter().all(|byte| *byte == 0)
        || remote_challenge.iter().all(|byte| *byte == 0)
        || local_challenge == remote_challenge
        || local_echo != remote_challenge
        || remote_echo != local_challenge
    {
        return Err("challenge reflection or mismatch");
    }
    if local_facts != local_ack_facts || remote_facts != remote_ack_facts {
        return Err("peer changed capability claims during handshake");
    }
    let mut inbound =
        VerifiedSessionSequence::new(network, remote, session).map_err(|_| "invalid session")?;
    inbound
        .accept(remote_hello)
        .map_err(|_| "invalid hello replay")?;
    inbound
        .accept(remote_ack)
        .map_err(|_| "invalid acknowledgement replay")?;
    Ok(remote_facts.clone())
}

/// Result of a bounded claim-only exchange. This is not authorization:
/// a caller must first establish mutually verified TLS 1.3, verify the pinned
/// peer certificate and approved (NetworkId, NodeId), and export the session
/// ID from that verified connection before using any returned peer facts.
#[derive(Debug)]
pub enum HelloExchangeError {
    Transport(ControlIoError),
    InvalidTranscript(&'static str),
}

impl From<ControlIoError> for HelloExchangeError {
    fn from(error: ControlIoError) -> Self {
        Self::Transport(error)
    }
}

impl fmt::Display for HelloExchangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "control hello I/O failed: {error}"),
            Self::InvalidTranscript(error) => write!(f, "invalid control hello: {error}"),
        }
    }
}

impl std::error::Error for HelloExchangeError {}

/// Execute a complete symmetric Hello / HelloAck transcript on a caller-owned
/// byte stream with one absolute deadline. Both peers write their Hello first,
/// then read their counterpart's Hello, then exchange the challenge echoes.
/// This function only returns *untrusted claims*, not an authenticated session.
///
/// The caller MUST have independently verified a mutual TLS peer certificate,
/// an enrolled key pin, logical node and network approval, and MUST supply an
/// exporter session ID from that very TLS channel. This helper cannot prove
/// those external preconditions or activate production JoinNetwork. Any error
/// requires discarding the transport; no plaintext fallback is permitted.
/// Caller-supplied preconditions and local hello fields. Construct this only
/// after verifying TLS peer identity and obtaining a fresh exporter and
/// cryptographically random challenge from a trusted source.
#[derive(Clone, Debug)]
pub struct HelloExchangeRequest {
    pub network: NetworkId,
    pub local: NodeId,
    pub remote: NodeId,
    pub exported_session_id: [u8; TLS_CONTROL_EXPORTER_BYTES],
    pub local_challenge: [u8; 16],
    pub local_facts: HelloFacts,
    pub deadline: Instant,
}

pub fn exchange_hello_claims_with_deadline<Stream>(
    stream: &mut Stream,
    request: HelloExchangeRequest,
) -> Result<HelloFacts, HelloExchangeError>
where
    Stream: DeadlineControlRead + DeadlineControlWrite,
{
    let HelloExchangeRequest {
        network,
        local,
        remote,
        exported_session_id,
        local_challenge,
        local_facts,
        deadline,
    } = request;
    if network == NetworkId::from_bytes([0; 16])
        || local == NodeId::from_bytes([0; 16])
        || remote == NodeId::from_bytes([0; 16])
    {
        return Err(HelloExchangeError::InvalidTranscript(
            "control exchange requires nonzero network and node identities",
        ));
    }
    if local == remote {
        return Err(HelloExchangeError::InvalidTranscript(
            "local and remote node must differ",
        ));
    }
    if exported_session_id.iter().all(|byte| *byte == 0) {
        return Err(HelloExchangeError::InvalidTranscript(
            "invalid zero TLS exporter session ID",
        ));
    }
    if local_challenge.iter().all(|byte| *byte == 0) {
        return Err(HelloExchangeError::InvalidTranscript(
            "invalid zero local hello challenge",
        ));
    }

    let local_hello = ControlEnvelope {
        version: crate::control::CONTROL_VERSION,
        network,
        sender: local,
        session_id: exported_session_id,
        sequence: 1,
        message: ControlMessage::Hello {
            challenge: local_challenge,
            facts: local_facts.clone(),
        },
    };
    write_one_control_frame_with_deadline(stream, &local_hello, deadline)?;

    let remote_hello = read_one_control_frame_with_deadline(stream, deadline)?;
    let remote_challenge = match &remote_hello.message {
        ControlMessage::Hello { challenge, .. }
            if remote_hello.network == network
                && remote_hello.sender == remote
                && remote_hello.session_id == exported_session_id
                && remote_hello.sequence == 1
                && *challenge != [0; 16]
                && *challenge != local_challenge =>
        {
            *challenge
        }
        _ => {
            return Err(HelloExchangeError::InvalidTranscript(
                "untrusted remote hello identity, session or challenge",
            ));
        }
    };

    let local_ack = ControlEnvelope {
        version: crate::control::CONTROL_VERSION,
        network,
        sender: local,
        session_id: exported_session_id,
        sequence: 2,
        message: ControlMessage::HelloAck {
            challenge: remote_challenge,
            facts: local_facts,
        },
    };
    write_one_control_frame_with_deadline(stream, &local_ack, deadline)?;
    let remote_ack = read_one_control_frame_with_deadline(stream, deadline)?;

    validate_exporter_bound_hello_transcript(
        network,
        local,
        remote,
        exported_session_id,
        &[local_hello, remote_hello, local_ack, remote_ack],
    )
    .map_err(HelloExchangeError::InvalidTranscript)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::CONTROL_VERSION;
    use blueroute_core::NodeCapabilities;

    fn frame(sender: u8, sequence: u64, message: ControlMessage) -> ControlEnvelope {
        ControlEnvelope {
            version: CONTROL_VERSION,
            network: NetworkId::from_bytes([1; 16]),
            sender: NodeId::from_bytes([sender; 16]),
            session_id: [3; 16],
            sequence,
            message,
        }
    }
    fn facts() -> HelloFacts {
        HelloFacts {
            software_version: "0.1.0".into(),
            capabilities: NodeCapabilities::default(),
            segment: None,
            direct_neighbors: vec![],
        }
    }
    fn exchange() -> [ControlEnvelope; 4] {
        [
            frame(
                4,
                1,
                ControlMessage::Hello {
                    challenge: [10; 16],
                    facts: facts(),
                },
            ),
            frame(
                5,
                1,
                ControlMessage::Hello {
                    challenge: [11; 16],
                    facts: facts(),
                },
            ),
            frame(
                4,
                2,
                ControlMessage::HelloAck {
                    challenge: [11; 16],
                    facts: facts(),
                },
            ),
            frame(
                5,
                2,
                ControlMessage::HelloAck {
                    challenge: [10; 16],
                    facts: facts(),
                },
            ),
        ]
    }
    fn verify(frames: &[ControlEnvelope; 4]) -> Result<HelloFacts, &'static str> {
        validate_hello_transcript(
            NetworkId::from_bytes([1; 16]),
            NodeId::from_bytes([4; 16]),
            NodeId::from_bytes([5; 16]),
            &frames[0],
            &frames[1],
            &frames[2],
            &frames[3],
        )
    }
    #[test]
    fn tls_exporter_context_is_direction_independent_and_network_scoped() {
        let network = NetworkId::from_bytes([1; 16]);
        let a = NodeId::from_bytes([4; 16]);
        let b = NodeId::from_bytes([5; 16]);
        let ab = tls_control_exporter_context(network, a, b).unwrap();
        assert_eq!(ab, tls_control_exporter_context(network, b, a).unwrap());
        assert_eq!(&ab[..16], network.as_bytes());
        assert_eq!(&ab[16..32], a.as_bytes());
        assert_eq!(&ab[32..], b.as_bytes());
        assert_ne!(
            ab,
            tls_control_exporter_context(NetworkId::from_bytes([2; 16]), a, b).unwrap()
        );
        assert!(tls_control_exporter_context(network, a, a).is_err());
        assert!(tls_control_exporter_context(NetworkId::from_bytes([0; 16]), a, b).is_err());
        assert!(tls_control_exporter_context(network, NodeId::from_bytes([0; 16]), b).is_err());
        assert!(tls_control_exporter_context(network, a, NodeId::from_bytes([0; 16])).is_err());
        assert_eq!(TLS_CONTROL_EXPORTER_BYTES, 16);
    }

    #[test]
    fn hello_transcript_must_match_actual_tls_exporter_session_id() {
        let frames = exchange();
        let network = NetworkId::from_bytes([1; 16]);
        let local = NodeId::from_bytes([4; 16]);
        let remote = NodeId::from_bytes([5; 16]);
        let verify_exporter = |exporter| {
            validate_exporter_bound_hello_transcript(network, local, remote, exporter, &frames)
        };
        assert_eq!(verify_exporter([3; 16]), Ok(facts()));
        assert!(verify_exporter([8; 16]).is_err());
        assert!(verify_exporter([0; 16]).is_err());
    }

    #[test]
    fn matching_bidirectional_hello_and_ack_are_accepted_as_claims_only() {
        assert_eq!(verify(&exchange()), Ok(facts()));
    }
    #[test]
    fn spoofed_network_node_session_or_sequence_is_rejected() {
        let mut frames = exchange();
        frames[1].network = NetworkId::from_bytes([2; 16]);
        assert!(verify(&frames).is_err());
        frames = exchange();
        frames[1].sender = NodeId::from_bytes([6; 16]);
        assert!(verify(&frames).is_err());
        frames = exchange();
        frames[3].session_id = [7; 16];
        assert!(verify(&frames).is_err());
        frames = exchange();
        frames[3].sequence = 1;
        assert!(verify(&frames).is_err());
    }
    #[test]
    fn reflection_wrong_ack_and_changed_claims_are_rejected() {
        let mut frames = exchange();
        frames[1].message = ControlMessage::Hello {
            challenge: [10; 16],
            facts: facts(),
        };
        assert!(verify(&frames).is_err());
        frames = exchange();
        frames[3].message = ControlMessage::HelloAck {
            challenge: [8; 16],
            facts: facts(),
        };
        assert!(verify(&frames).is_err());
        frames = exchange();
        let mut altered = facts();
        altered.software_version = "2.0".into();
        frames[3].message = ControlMessage::HelloAck {
            challenge: [10; 16],
            facts: altered,
        };
        assert!(verify(&frames).is_err());
    }
    /// The fixture is a byte transport, not TLS. It demonstrates only frame
    /// ordering, deadline calls and claim validation. Never use as auth proof.
    struct ScriptedTransport {
        inbound: std::io::Cursor<Vec<u8>>,
        outbound: Vec<u8>,
        read_timeouts: usize,
        write_timeouts: usize,
    }

    impl std::io::Read for ScriptedTransport {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            std::io::Read::read(&mut self.inbound, buffer)
        }
    }

    impl std::io::Write for ScriptedTransport {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.outbound.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl DeadlineControlRead for ScriptedTransport {
        fn set_control_read_timeout(&mut self, _: std::time::Duration) -> std::io::Result<()> {
            self.read_timeouts += 1;
            Ok(())
        }
    }

    impl DeadlineControlWrite for ScriptedTransport {
        fn set_control_write_timeout(&mut self, _: std::time::Duration) -> std::io::Result<()> {
            self.write_timeouts += 1;
            Ok(())
        }
    }

    fn scripted_transcript(
        remote_hello: ControlEnvelope,
        remote_ack: ControlEnvelope,
    ) -> ScriptedTransport {
        let mut inbound = encode_control_frame(&remote_hello).unwrap();
        inbound.extend(encode_control_frame(&remote_ack).unwrap());
        ScriptedTransport {
            inbound: std::io::Cursor::new(inbound),
            outbound: Vec::new(),
            read_timeouts: 0,
            write_timeouts: 0,
        }
    }

    fn exchange_claims(stream: &mut ScriptedTransport) -> Result<HelloFacts, HelloExchangeError> {
        exchange_hello_claims_with_deadline(
            stream,
            HelloExchangeRequest {
                network: NetworkId::from_bytes([1; 16]),
                local: NodeId::from_bytes([4; 16]),
                remote: NodeId::from_bytes([5; 16]),
                exported_session_id: [3; 16],
                local_challenge: [10; 16],
                local_facts: facts(),
                deadline: Instant::now() + std::time::Duration::from_secs(2),
            },
        )
    }

    #[test]
    fn framed_claim_exchange_respects_deadlines_and_challenge_echo_order() {
        let frames = exchange();
        let mut stream = scripted_transcript(frames[1].clone(), frames[3].clone());
        assert_eq!(exchange_claims(&mut stream).unwrap(), facts());
        assert!(stream.read_timeouts > 0);
        assert!(stream.write_timeouts > 0);
        let mut recorded = ScriptedTransport {
            inbound: std::io::Cursor::new(stream.outbound),
            outbound: Vec::new(),
            read_timeouts: 0,
            write_timeouts: 0,
        };
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        assert_eq!(
            read_one_control_frame_with_deadline(&mut recorded, deadline).unwrap(),
            frames[0]
        );
        assert_eq!(
            read_one_control_frame_with_deadline(&mut recorded, deadline).unwrap(),
            frames[2]
        );
    }

    #[test]
    fn malformed_remote_identity_is_rejected_before_echoing_peer_challenge() {
        for mutate in 0..5 {
            let mut frames = exchange();
            match mutate {
                0 => frames[1].network = NetworkId::from_bytes([9; 16]),
                1 => frames[1].sender = NodeId::from_bytes([9; 16]),
                2 => frames[1].session_id = [9; 16],
                3 => frames[1].sequence = 3,
                _ => {
                    // A zero challenge is rejected by the frame encoder; use
                    // a well-formed challenge reflection to test the handshake.
                    frames[1].message = ControlMessage::Hello {
                        challenge: [10; 16],
                        facts: facts(),
                    }
                }
            }
            let mut stream = scripted_transcript(frames[1].clone(), frames[3].clone());
            assert!(matches!(
                exchange_claims(&mut stream),
                Err(HelloExchangeError::InvalidTranscript(_))
            ));
            let first_bytes = encode_control_frame(&exchange()[0]).unwrap();
            assert_eq!(
                stream.outbound, first_bytes,
                "no ACK may be emitted for an invalid peer hello"
            );
        }
    }

    #[test]
    fn invalid_remote_ack_and_incomplete_transcript_are_rejected() {
        let frames = exchange();
        let mut bad_ack = frames[3].clone();
        bad_ack.message = ControlMessage::HelloAck {
            challenge: [99; 16],
            facts: facts(),
        };
        let mut stream = scripted_transcript(frames[1].clone(), bad_ack);
        assert!(matches!(
            exchange_claims(&mut stream),
            Err(HelloExchangeError::InvalidTranscript(_))
        ));
        let mut stream = scripted_transcript(frames[1].clone(), frames[3].clone());
        let truncated_len = stream.inbound.get_ref().len() - 2;
        stream.inbound.get_mut().truncate(truncated_len);
        assert!(matches!(
            exchange_claims(&mut stream),
            Err(HelloExchangeError::Transport(_))
        ));
    }

    #[test]
    fn expired_budget_rejects_transcript_before_emitting_bytes() {
        let frames = exchange();
        let mut stream = scripted_transcript(frames[1].clone(), frames[3].clone());
        let result = exchange_hello_claims_with_deadline(
            &mut stream,
            HelloExchangeRequest {
                network: NetworkId::from_bytes([1; 16]),
                local: NodeId::from_bytes([4; 16]),
                remote: NodeId::from_bytes([5; 16]),
                exported_session_id: [3; 16],
                local_challenge: [10; 16],
                local_facts: facts(),
                deadline: Instant::now() - std::time::Duration::from_secs(1),
            },
        );
        assert!(matches!(result, Err(HelloExchangeError::Transport(_))));
        assert!(stream.outbound.is_empty());
    }


    #[test]
    fn zero_identity_handshake_preflight_never_emits_unauthenticated_bytes() {
        let frames = exchange();
        for case in 0..3 {
            let mut stream = scripted_transcript(frames[1].clone(), frames[3].clone());
            let mut request = HelloExchangeRequest {
                network: NetworkId::from_bytes([1; 16]),
                local: NodeId::from_bytes([4; 16]),
                remote: NodeId::from_bytes([5; 16]),
                exported_session_id: [3; 16],
                local_challenge: [10; 16],
                local_facts: facts(),
                deadline: Instant::now() + std::time::Duration::from_secs(2),
            };
            match case {
                0 => request.network = NetworkId::from_bytes([0; 16]),
                1 => request.local = NodeId::from_bytes([0; 16]),
                _ => request.remote = NodeId::from_bytes([0; 16]),
            }
            assert!(matches!(
                exchange_hello_claims_with_deadline(&mut stream, request),
                Err(HelloExchangeError::InvalidTranscript(_))
            ));
            assert!(stream.outbound.is_empty(), "unexpected output for case {case}");
            assert_eq!(stream.write_timeouts, 0);
            assert_eq!(stream.read_timeouts, 0);
        }
    }

    #[test]
    fn zero_challenges_are_rejected_without_authorizing_claims() {
        let mut frames = exchange();
        frames[1].message = ControlMessage::Hello {
            challenge: [0; 16],
            facts: facts(),
        };
        assert!(verify(&frames).is_err());
        frames = exchange();
        frames[0].message = ControlMessage::Hello {
            challenge: [0; 16],
            facts: facts(),
        };
        assert!(verify(&frames).is_err());
    }

    #[test]
    fn unknown_version_and_non_hello_messages_fail_closed() {
        let mut frames = exchange();
        frames[1].version += 1;
        assert!(verify(&frames).is_err());
        frames = exchange();
        frames[3].message = ControlMessage::Heartbeat;
        assert!(verify(&frames).is_err());
    }
}

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
    local_hello: &ControlEnvelope,
    remote_hello: &ControlEnvelope,
    local_ack: &ControlEnvelope,
    remote_ack: &ControlEnvelope,
) -> Result<HelloFacts, &'static str> {
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
    if local_challenge == remote_challenge
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
        assert_eq!(TLS_CONTROL_EXPORTER_BYTES, 16);
    }

    #[test]
    fn hello_transcript_must_match_actual_tls_exporter_session_id() {
        let frames = exchange();
        let network = NetworkId::from_bytes([1; 16]);
        let local = NodeId::from_bytes([4; 16]);
        let remote = NodeId::from_bytes([5; 16]);
        let verify_exporter = |exporter| {
            validate_exporter_bound_hello_transcript(
                network, local, remote, exporter, &frames[0], &frames[1], &frames[2], &frames[3],
            )
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

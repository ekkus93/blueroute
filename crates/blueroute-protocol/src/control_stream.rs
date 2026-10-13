//! Bounded incremental control frame decoder for TLS byte streams.
//!
//! This parser is NOT an authenticator. Never dispatch its decoded messages
//! to membership/route mutations without a separately verified mutual TLS
//! peer identity, network approval, channel binding and replay protection.

use crate::control::{
    ControlEnvelope, MAX_CONTROL_FRAME_BYTES, decode_control_frame, read_control_length,
};

const HEADER_BYTES: usize = 4;
pub const MAX_STREAM_FEED_BYTES: usize = 64 * 1024;
pub const MAX_FRAMES_PER_FEED: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamDecodeError {
    ChunkTooLarge,
    FrameLimit,
    MalformedFrame,
    Poisoned,
}

/// Each TLS connection owns a fresh decoder; on any protocol violation the
/// connection must close, and this decoder can never recover or resynchronize.
#[derive(Debug, Default)]
pub struct ControlStreamDecoder {
    partial: Vec<u8>,
    expected: Option<usize>,
    poisoned: bool,
}

impl ControlStreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_bytes(&self) -> usize {
        self.partial.len()
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<ControlEnvelope>, StreamDecodeError> {
        if self.poisoned {
            return Err(StreamDecodeError::Poisoned);
        }
        if chunk.len() > MAX_STREAM_FEED_BYTES {
            return self.reject(StreamDecodeError::ChunkTooLarge);
        }
        let mut input = chunk;
        let mut completed = Vec::new();
        while !input.is_empty() {
            let target = self.expected.unwrap_or(HEADER_BYTES);
            let needed = target - self.partial.len();
            let count = needed.min(input.len());
            self.partial.extend_from_slice(&input[..count]);
            input = &input[count..];
            if self.partial.len() != target {
                continue;
            }
            if self.expected.is_none() {
                let size = match read_control_length(&self.partial) {
                    Ok(size) => size,
                    Err(_) => return self.reject(StreamDecodeError::MalformedFrame),
                };
                self.expected = Some(HEADER_BYTES + size);
                continue;
            }
            let envelope = match decode_control_frame(&self.partial) {
                Ok(frame) => frame,
                Err(_) => return self.reject(StreamDecodeError::MalformedFrame),
            };
            if completed.len() >= MAX_FRAMES_PER_FEED {
                return self.reject(StreamDecodeError::FrameLimit);
            }
            completed.push(envelope);
            self.partial.clear();
            self.expected = None;
        }
        debug_assert!(self.partial.len() <= HEADER_BYTES + MAX_CONTROL_FRAME_BYTES);
        Ok(completed)
    }

    fn reject<T>(&mut self, error: StreamDecodeError) -> Result<T, StreamDecodeError> {
        self.partial.clear();
        self.expected = None;
        self.poisoned = true;
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{CONTROL_VERSION, ControlMessage, encode_control_frame};
    use blueroute_core::{NetworkId, NodeId};

    fn example(sequence: u64) -> ControlEnvelope {
        ControlEnvelope {
            version: CONTROL_VERSION,
            network: NetworkId::from_bytes([1; 16]),
            sender: NodeId::from_bytes([2; 16]),
            session_id: [3; 16],
            sequence,
            message: ControlMessage::Heartbeat,
        }
    }

    #[test]
    fn split_header_and_payload_into_every_single_byte() {
        let envelope = example(1);
        let bytes = encode_control_frame(&envelope).unwrap();
        let mut decoder = ControlStreamDecoder::new();
        for byte in &bytes[..bytes.len() - 1] {
            assert!(decoder.feed(&[*byte]).unwrap().is_empty());
        }
        assert_eq!(
            decoder.feed(&bytes[bytes.len() - 1..]).unwrap(),
            vec![envelope]
        );
        assert_eq!(decoder.pending_bytes(), 0);
    }

    #[test]
    fn coalesced_frames_and_fragmented_tail_decode_in_order() {
        let first = example(1);
        let second = example(2);
        let third = example(3);
        let mut bytes = encode_control_frame(&first).unwrap();
        bytes.extend(encode_control_frame(&second).unwrap());
        let tail = encode_control_frame(&third).unwrap();
        bytes.extend(&tail[..8]);
        let mut decoder = ControlStreamDecoder::new();
        assert_eq!(decoder.feed(&bytes).unwrap(), vec![first, second]);
        assert_eq!(decoder.feed(&tail[8..]).unwrap(), vec![third]);
    }

    #[test]
    fn invalid_length_and_payload_poison_connection() {
        for bytes in [
            vec![0, 0, 0, 0],
            ((MAX_CONTROL_FRAME_BYTES as u32) + 1)
                .to_be_bytes()
                .to_vec(),
            vec![0, 0, 0, 1, 0xff],
        ] {
            let mut decoder = ControlStreamDecoder::new();
            assert_eq!(decoder.feed(&bytes), Err(StreamDecodeError::MalformedFrame));
            assert!(decoder.is_poisoned());
            assert_eq!(
                decoder.feed(&encode_control_frame(&example(1)).unwrap()),
                Err(StreamDecodeError::Poisoned)
            );
            assert_eq!(decoder.pending_bytes(), 0);
        }
    }

    #[test]
    fn oversized_feed_and_frame_flood_fail_closed() {
        let mut decoder = ControlStreamDecoder::new();
        assert_eq!(
            decoder.feed(&vec![0; MAX_STREAM_FEED_BYTES + 1]),
            Err(StreamDecodeError::ChunkTooLarge)
        );
        let mut flood = Vec::new();
        for sequence in 1..=(MAX_FRAMES_PER_FEED + 1) {
            flood.extend(encode_control_frame(&example(sequence as u64)).unwrap());
        }
        let mut decoder = ControlStreamDecoder::new();
        assert_eq!(decoder.feed(&flood), Err(StreamDecodeError::FrameLimit));
        assert!(decoder.is_poisoned());
    }

    #[test]
    fn bounded_random_chunks_never_panic_or_grow_unbounded() {
        let mut seed = 0xabc123_u32;
        for _ in 0..256 {
            let mut decoder = ControlStreamDecoder::new();
            let mut bytes = [0_u8; 128];
            for byte in &mut bytes {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            let _ = decoder.feed(&bytes);
            assert!(decoder.pending_bytes() <= HEADER_BYTES + MAX_CONTROL_FRAME_BYTES);
        }
    }
}

//! Deterministic hostile-stream tests for P7-005 parser groundwork.
//!
//! These tests exercise untrusted frame parsing only. They do NOT establish
//! TLS authentication, authorize membership, or qualify Bluetooth hardware.

use std::io::{self, Cursor, ErrorKind, Read, Write};
use std::panic::catch_unwind;

use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::control::{
    CONTROL_VERSION, ControlEnvelope, ControlMessage, MAX_CONTROL_FRAME_BYTES,
    encode_control_frame,
};
use blueroute_protocol::control_io::{
    ControlIoError, read_one_control_frame, write_one_control_frame,
};

fn heartbeat() -> ControlEnvelope {
    ControlEnvelope {
        version: CONTROL_VERSION,
        network: NetworkId::from_bytes([1; 16]),
        sender: NodeId::from_bytes([2; 16]),
        session_id: [3; 16],
        sequence: 1,
        message: ControlMessage::Heartbeat,
    }
}

#[test]
fn every_truncation_of_a_valid_frame_fails_without_panic() {
    let encoded = encode_control_frame(&heartbeat()).unwrap();
    for length in 0..encoded.len() {
        let result = catch_unwind(|| {
            read_one_control_frame(&mut Cursor::new(&encoded[..length]))
        });
        assert!(result.is_ok(), "parser panicked at prefix length {length}");
        assert!(result.unwrap().is_err(), "truncated frame accepted at {length}");
    }
    assert_eq!(
        read_one_control_frame(&mut Cursor::new(encoded)).unwrap(),
        heartbeat()
    );
}

#[test]
fn deterministic_mutated_wire_data_never_panics() {
    let original = encode_control_frame(&heartbeat()).unwrap();
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    for iteration in 0..2048_usize {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mut bytes = original.clone();
        let offset = (seed as usize) % bytes.len();
        bytes[offset] ^= (seed >> 32) as u8 | 1;
        if iteration % 7 == 0 {
            bytes.truncate((seed as usize) % bytes.len());
        }
        let result = catch_unwind(|| {
            read_one_control_frame(&mut Cursor::new(bytes))
        });
        assert!(result.is_ok(), "parser panicked on mutation {iteration}");
    }
}

#[test]
fn adversarial_length_prefixes_are_bounded_before_body_reads() {
    for size in [0_u32, 1, MAX_CONTROL_FRAME_BYTES as u32, (MAX_CONTROL_FRAME_BYTES + 1) as u32, u32::MAX] {
        let prefix = size.to_be_bytes();
        let result = read_one_control_frame(&mut Cursor::new(prefix));
        if size == 0 || size as usize > MAX_CONTROL_FRAME_BYTES {
            assert!(matches!(result, Err(ControlIoError::Frame(_))));
        } else {
            assert!(matches!(result, Err(ControlIoError::Io(ref e)) if e.kind() == ErrorKind::UnexpectedEof));
        }
    }
}

struct InterruptedOnce<R> {
    inner: R,
    interrupted: bool,
}

impl<R: Read> Read for InterruptedOnce<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::from(ErrorKind::Interrupted));
        }
        self.inner.read(buf)
    }
}

struct InterruptedOnceWriter<W> {
    inner: W,
    interrupted: bool,
}

impl<W: Write> Write for InterruptedOnceWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::from(ErrorKind::Interrupted));
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[test]
fn interrupted_reads_and_writes_retry_without_losing_frame_bytes() {
    let mut writer = InterruptedOnceWriter {
        inner: Vec::new(),
        interrupted: false,
    };
    write_one_control_frame(&mut writer, &heartbeat()).unwrap();
    assert!(writer.interrupted);
    let mut reader = InterruptedOnce {
        inner: Cursor::new(writer.inner),
        interrupted: false,
    };
    assert_eq!(read_one_control_frame(&mut reader).unwrap(), heartbeat());
    assert!(reader.interrupted);
}

struct BrokenWriter {
    written: usize,
    fail_after: usize,
}

impl Write for BrokenWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.written >= self.fail_after {
            return Err(io::Error::from(ErrorKind::BrokenPipe));
        }
        let count = bytes.len().min(1);
        self.written += count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn partial_outbound_frame_propagates_write_failure() {
    let mut writer = BrokenWriter {
        written: 0,
        fail_after: 7,
    };
    assert!(matches!(
        write_one_control_frame(&mut writer, &heartbeat()),
        Err(ControlIoError::Io(ref error)) if error.kind() == ErrorKind::BrokenPipe
    ));
    assert_eq!(writer.written, 7);
}

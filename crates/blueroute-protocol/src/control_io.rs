//! Bounded frame I/O for future verified mutual-TLS byte streams.
//! No authentication is performed here; decoded claims must never authorize
//! peer membership, routing or a production JoinNetwork.

use std::fmt;
use std::io::{self, Read, Write};

use crate::control::{
    ControlEnvelope, ControlFrameError, decode_control_frame, encode_control_frame,
    read_control_length,
};

#[derive(Debug)]
pub enum ControlIoError {
    Io(io::Error),
    Frame(ControlFrameError),
}

impl fmt::Display for ControlIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "control I/O failure: {error}"),
            Self::Frame(error) => write!(f, "invalid control frame: {error}"),
        }
    }
}

impl std::error::Error for ControlIoError {}

impl From<io::Error> for ControlIoError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ControlFrameError> for ControlIoError {
    fn from(error: ControlFrameError) -> Self {
        Self::Frame(error)
    }
}

/// Read exactly one control frame, validating the four-byte prefix before
/// allocating any body bytes. On error the stream MUST be closed; a caller
/// must not try to resynchronize an untrusted peer's malformed TLS record.
pub fn read_one_control_frame<R: Read>(source: &mut R) -> Result<ControlEnvelope, ControlIoError> {
    let mut prefix = [0_u8; 4];
    source.read_exact(&mut prefix)?;
    let length = read_control_length(&prefix)?;
    let mut bytes = vec![0_u8; length + 4];
    bytes[..4].copy_from_slice(&prefix);
    source.read_exact(&mut bytes[4..])?;
    Ok(decode_control_frame(&bytes)?)
}

/// Write one locally validated frame, handling partial writes. This must be
/// called only over a separately authenticated and channel-bound TLS stream.
pub fn write_one_control_frame<W: Write>(
    destination: &mut W,
    frame: &ControlEnvelope,
) -> Result<(), ControlIoError> {
    let bytes = encode_control_frame(frame)?;
    destination.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind, Read, Write};
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    use blueroute_core::{NetworkId, NodeId};

    use super::*;
    use crate::control::{CONTROL_VERSION, ControlMessage, MAX_CONTROL_FRAME_BYTES};

    fn message(sequence: u64) -> ControlEnvelope {
        ControlEnvelope {
            version: CONTROL_VERSION,
            network: NetworkId::from_bytes([1; 16]),
            sender: NodeId::from_bytes([2; 16]),
            session_id: [3; 16],
            sequence,
            message: ControlMessage::Heartbeat,
        }
    }

    struct ByteWise<T>(T);

    impl<T: Read> Read for ByteWise<T> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(&mut buf[..buf.len().min(1)])
        }
    }

    impl<T: Write> Write for ByteWise<T> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(&buf[..buf.len().min(1)])
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    #[test]
    fn fragmented_reads_and_writes_round_trip() {
        let mut writer = ByteWise(Vec::new());
        write_one_control_frame(&mut writer, &message(1)).unwrap();
        write_one_control_frame(&mut writer, &message(2)).unwrap();
        let mut reader = ByteWise(Cursor::new(writer.0));
        assert_eq!(read_one_control_frame(&mut reader).unwrap(), message(1));
        assert_eq!(read_one_control_frame(&mut reader).unwrap(), message(2));
        assert!(matches!(
            read_one_control_frame(&mut reader),
            Err(ControlIoError::Io(ref e)) if e.kind() == ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn oversized_header_is_rejected_without_reading_or_allocating_body() {
        let header = ((MAX_CONTROL_FRAME_BYTES as u32) + 1).to_be_bytes();
        let mut input = Cursor::new(header);
        assert!(matches!(
            read_one_control_frame(&mut input),
            Err(ControlIoError::Frame(_))
        ));
        assert_eq!(input.position(), 4);
    }

    #[test]
    fn rejects_truncated_and_invalid_frames_without_writing_invalid_output() {
        let mut empty = Cursor::new([0_u8; 2]);
        assert!(matches!(
            read_one_control_frame(&mut empty),
            Err(ControlIoError::Io(_))
        ));
        let bytes = encode_control_frame(&message(1)).unwrap();
        let mut partial = Cursor::new(&bytes[..bytes.len() - 1]);
        assert!(matches!(
            read_one_control_frame(&mut partial),
            Err(ControlIoError::Io(_))
        ));
        let mut malformed = Cursor::new([0_u8, 0, 0, 1, 0xff]);
        assert!(matches!(
            read_one_control_frame(&mut malformed),
            Err(ControlIoError::Frame(_))
        ));
        let mut invalid = message(0);
        let mut output = Vec::new();
        assert!(write_one_control_frame(&mut output, &invalid).is_err());
        assert!(output.is_empty());
        invalid.sequence = 1;
        write_one_control_frame(&mut output, &invalid).unwrap();
        assert_eq!(output, bytes);
    }

    #[test]
    fn localhost_tcp_exercises_framing_but_not_authentication() {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_one_control_frame(&mut stream).unwrap(), message(1));
            write_one_control_frame(&mut stream, &message(2)).unwrap();
        });
        let mut client = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write_one_control_frame(&mut client, &message(1)).unwrap();
        assert_eq!(read_one_control_frame(&mut client).unwrap(), message(2));
        server.join().unwrap();
    }
}

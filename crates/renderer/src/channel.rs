//! Length-prefixed frames for the renderer platform channel.
//!
//! [ADR 0019](../../../docs/adrs/0019-async-browser-runtime-and-io.md): one
//! private full-duplex channel per renderer process. Every frame starts with a
//! fixed 16-byte header:
//!
//! ```text
//! version: u8 | kind: u8 | flags: u16 (zero) | request id: u64 | length: u32
//! ```
//!
//! The version byte is [`PROTOCOL_VERSION`]; a mismatch fails closed. Control
//! payloads are JSON and capped at [`MAX_CONTROL_BYTES`]. Body chunks are raw
//! bytes and capped at [`MAX_BODY_CHUNK_BYTES`]. Readers validate the header and
//! length before they allocate or read the payload, so a corrupt or hostile
//! peer cannot force an unbounded allocation.
//!
//! The first frame in each direction is the handshake: the browser sends
//! [`ToRenderer::Hello`](crate::ToRenderer::Hello) and the renderer replies
//! [`FromRenderer::Ready`](crate::FromRenderer::Ready).

use std::io::{self, Read, Write};

use serde::{Serialize, de::DeserializeOwned};

/// Framing and message ABI version for the renderer channel.
pub const PROTOCOL_VERSION: u8 = 1;

/// Fixed frame header size in bytes.
pub const HEADER_BYTES: usize = 16;

/// Maximum encoded size of one JSON control payload.
pub const MAX_CONTROL_BYTES: usize = 8 * 1024 * 1024;

/// Maximum size of one raw response-body chunk.
pub const MAX_BODY_CHUNK_BYTES: usize = 64 * 1024;

const KIND_CONTROL: u8 = 0;
const KIND_BODY: u8 = 1;

/// Payload class of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// JSON control message.
    Control,
    /// Raw response-body chunk.
    Body,
}

impl FrameKind {
    fn wire(self) -> u8 {
        match self {
            Self::Control => KIND_CONTROL,
            Self::Body => KIND_BODY,
        }
    }

    fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            KIND_CONTROL => Some(Self::Control),
            KIND_BODY => Some(Self::Body),
            _ => None,
        }
    }

    fn max_payload(self) -> usize {
        match self {
            Self::Control => MAX_CONTROL_BYTES,
            Self::Body => MAX_BODY_CHUNK_BYTES,
        }
    }
}

/// One decoded frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Payload class.
    pub kind: FrameKind,
    /// Request this frame belongs to; zero for control frames.
    pub request: u64,
}

/// Writes one control message as a JSON frame.
///
/// # Errors
///
/// Serialization failure or a payload larger than [`MAX_CONTROL_BYTES`].
pub fn write_control<T: Serialize>(writer: &mut impl Write, message: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    write_frame(writer, FrameKind::Control, 0, &payload)
}

/// Writes one raw body chunk for `request`.
///
/// # Errors
///
/// A payload larger than [`MAX_BODY_CHUNK_BYTES`] or write failure.
pub fn write_body(writer: &mut impl Write, request: u64, payload: &[u8]) -> io::Result<()> {
    write_frame(writer, FrameKind::Body, request, payload)
}

/// Writes one frame with the fixed header.
///
/// # Errors
///
/// A payload larger than the kind limit or write failure.
pub fn write_frame(
    writer: &mut impl Write,
    kind: FrameKind,
    request: u64,
    payload: &[u8],
) -> io::Result<()> {
    if payload.len() > kind.max_payload() {
        return Err(invalid("renderer IPC payload exceeds limit"));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| invalid("renderer IPC payload exceeds limit"))?;
    let mut header = [0u8; HEADER_BYTES];
    header[0] = PROTOCOL_VERSION;
    header[1] = kind.wire();
    // header[2..4] is a zero flags field; readers reject nonzero values.
    header[4..12].copy_from_slice(&request.to_be_bytes());
    header[12..16].copy_from_slice(&length.to_be_bytes());
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Reads one frame header and payload into `payload`.
///
/// `Ok(None)` is a clean peer close at a frame boundary. Every other failure is
/// deterministic: an unknown version or kind, nonzero flags, an oversized
/// length, or a truncated header or payload leaves the channel unusable.
///
/// # Errors
///
/// I/O failure or a protocol violation.
pub fn read_frame(reader: &mut impl Read, payload: &mut Vec<u8>) -> io::Result<Option<Frame>> {
    let mut header = [0u8; HEADER_BYTES];
    if reader.read(&mut header[..1])? == 0 {
        return Ok(None);
    }
    read_exact(reader, &mut header[1..])?;
    if header[0] != PROTOCOL_VERSION {
        return Err(invalid("unsupported renderer IPC version"));
    }
    let kind = FrameKind::from_wire(header[1])
        .ok_or_else(|| invalid("unknown renderer IPC frame kind"))?;
    if header[2] != 0 || header[3] != 0 {
        return Err(invalid("nonzero renderer IPC flags"));
    }
    let request = u64::from_be_bytes([
        header[4], header[5], header[6], header[7], header[8], header[9], header[10], header[11],
    ]);
    let length = u32::from_be_bytes([header[12], header[13], header[14], header[15]]) as usize;
    if length > kind.max_payload() {
        return Err(invalid("renderer IPC frame exceeds limit"));
    }
    payload.clear();
    payload.resize(length, 0);
    read_exact(reader, payload)?;
    Ok(Some(Frame { kind, request }))
}

/// Reads one control message.
///
/// # Errors
///
/// I/O failure, a body frame, invalid JSON, or a protocol violation.
pub fn read_control<T: DeserializeOwned>(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
) -> io::Result<Option<T>> {
    match read_frame(reader, buffer)? {
        None => Ok(None),
        Some(Frame {
            kind: FrameKind::Control,
            ..
        }) => serde_json::from_slice(buffer)
            .map(Some)
            .map_err(|error| invalid(format!("invalid renderer IPC JSON: {error}"))),
        Some(_) => Err(invalid("expected a control frame")),
    }
}

/// Reads one raw body chunk, returning its request id.
///
/// # Errors
///
/// I/O failure, a control frame, or a protocol violation.
pub fn read_body(reader: &mut impl Read, buffer: &mut Vec<u8>) -> io::Result<Option<u64>> {
    match read_frame(reader, buffer)? {
        None => Ok(None),
        Some(Frame {
            kind: FrameKind::Body,
            request,
        }) => Ok(Some(request)),
        Some(_) => Err(invalid("expected a body frame")),
    }
}

fn read_exact(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<()> {
    reader
        .read_exact(buffer)
        .map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => invalid("truncated renderer IPC frame"),
            _ => error,
        })
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn header(kind: u8, flags: u16, request: u64, length: u32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_BYTES);
        bytes.push(PROTOCOL_VERSION);
        bytes.push(kind);
        bytes.extend_from_slice(&flags.to_be_bytes());
        bytes.extend_from_slice(&request.to_be_bytes());
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes
    }

    #[test]
    fn control_frames_round_trip() {
        let message = crate::ToRenderer::Request {
            id: 7,
            command: crate::Command::Eval {
                frame: crate::FrameId::MAIN,
                source: "1+1".into(),
            },
        };
        let mut bytes = Vec::new();
        write_control(&mut bytes, &message).expect("write");
        let mut reader = Cursor::new(bytes);
        let mut buffer = Vec::new();
        let back: crate::ToRenderer = read_control(&mut reader, &mut buffer)
            .expect("read")
            .expect("one frame");
        assert!(matches!(back, crate::ToRenderer::Request { id: 7, .. }));
    }

    #[test]
    fn body_frames_round_trip_at_the_cap() {
        let payload = vec![7u8; MAX_BODY_CHUNK_BYTES];
        let mut bytes = Vec::new();
        write_body(&mut bytes, 42, &payload).expect("write");
        let mut reader = Cursor::new(bytes);
        let mut buffer = Vec::new();
        let request = read_body(&mut reader, &mut buffer)
            .expect("read")
            .expect("one frame");
        assert_eq!(request, 42);
        assert_eq!(buffer, payload);
        assert!(
            read_body(&mut reader, &mut buffer)
                .expect("clean close")
                .is_none()
        );
    }

    #[test]
    fn oversized_payloads_fail_before_write() {
        let error = write_body(&mut Vec::new(), 1, &vec![0u8; MAX_BODY_CHUNK_BYTES + 1])
            .expect_err("oversized body");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let message = crate::ToRenderer::Request {
            id: 1,
            command: crate::Command::Eval {
                frame: crate::FrameId::MAIN,
                source: "x".repeat(MAX_CONTROL_BYTES),
            },
        };
        let error = write_control(&mut Vec::new(), &message).expect_err("oversized control");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn oversized_lengths_fail_before_allocation() {
        for (kind, length) in [
            (
                KIND_CONTROL,
                u32::try_from(MAX_CONTROL_BYTES + 1).expect("fits"),
            ),
            (
                KIND_BODY,
                u32::try_from(MAX_BODY_CHUNK_BYTES + 1).expect("fits"),
            ),
        ] {
            let mut reader = Cursor::new(header(kind, 0, 0, length));
            let mut buffer = Vec::new();
            let error = read_frame(&mut reader, &mut buffer).expect_err("oversized length");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn malformed_headers_fail_closed() {
        let mut buffer = Vec::new();

        let mut bad_version = header(KIND_CONTROL, 0, 0, 0);
        bad_version[0] = PROTOCOL_VERSION + 1;
        assert!(read_frame(&mut Cursor::new(bad_version), &mut buffer).is_err());

        assert!(read_frame(&mut Cursor::new(header(9, 0, 0, 0)), &mut buffer).is_err());
        assert!(read_frame(&mut Cursor::new(header(KIND_CONTROL, 1, 0, 0)), &mut buffer).is_err());

        assert!(
            read_control::<serde_json::Value>(
                &mut Cursor::new(header(KIND_BODY, 0, 1, 0)),
                &mut buffer
            )
            .is_err()
        );
        assert!(read_body(&mut Cursor::new(header(KIND_CONTROL, 0, 0, 0)), &mut buffer).is_err());
    }

    #[test]
    fn truncated_and_invalid_control_frames_fail_closed() {
        let mut buffer = Vec::new();

        let mut partial = header(KIND_CONTROL, 0, 0, 4);
        partial.push(b'{');
        assert!(read_frame(&mut Cursor::new(partial), &mut buffer).is_err());

        let mut invalid_json = header(KIND_CONTROL, 0, 0, 1);
        invalid_json.push(b'{');
        assert!(
            read_control::<serde_json::Value>(&mut Cursor::new(invalid_json), &mut buffer).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn closed_channel_reports_a_clean_close() {
        use std::os::unix::net::UnixStream;

        let (mut host, child) = UnixStream::pair().expect("socket pair");
        let mut writer = child.try_clone().expect("clone");
        write_body(&mut writer, 3, b"abc").expect("write");
        drop(writer);
        drop(child);

        let mut buffer = Vec::new();
        let request = read_body(&mut host, &mut buffer)
            .expect("read")
            .expect("one frame");
        assert_eq!(request, 3);
        assert_eq!(buffer, b"abc");
        assert!(
            read_frame(&mut host, &mut buffer)
                .expect("closed channel")
                .is_none()
        );
    }
}

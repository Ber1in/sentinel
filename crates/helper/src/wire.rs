//! Frame I/O over a blocking byte stream, plus the shared socket writer.
//!
//! `frame` only turns messages into bytes; this module deals with the stream
//! semantics (length prefix first, then the payload) and with the fact that
//! several threads may want to write to the same connection.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

use crate::frame::{self, Message, MAX_FRAME};

/// Read exactly one frame.
///
/// Returns `Ok(None)` on a clean EOF at a frame boundary. A connection that
/// dies mid-frame is reported as `UnexpectedEof`: the client never sends a
/// partial frame on purpose, and the handler treats both as "connection closed".
pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<(Message, Vec<u8>)>> {
    let mut len = [0u8; 4];
    loop {
        match reader.read(&mut len[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    reader.read_exact(&mut len[1..])?;

    let total = u32::from_be_bytes(len) as usize;
    if total > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large: {total} bytes"),
        ));
    }

    let mut raw = Vec::with_capacity(4 + total);
    raw.extend_from_slice(&len);
    raw.resize(4 + total, 0);
    reader.read_exact(&mut raw[4..])?;

    let (message, body) =
        frame::decode(&raw).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(Some((message, body)))
}

/// Write one frame to `writer` and flush it.
pub fn write_frame<W: Write>(writer: &mut W, message: &Message, body: &[u8]) -> io::Result<()> {
    let bytes = frame::encode(message, body);
    writer.write_all(&bytes)?;
    writer.flush()
}

/// A Unix socket that multiple threads may write frames to.
///
/// All frames for one connection go through a single mutex so a reply written
/// by the request handler can never interleave with terminal data written by a
/// session reader thread.
pub struct ConnectionOut {
    socket: Mutex<UnixStream>,
}

impl ConnectionOut {
    /// Wrap an accepted or connected socket.
    pub fn new(socket: UnixStream) -> Self {
        Self {
            socket: Mutex::new(socket),
        }
    }

    /// Send one frame. Errors usually mean the peer went away.
    pub fn send(&self, message: &Message, body: &[u8]) -> io::Result<()> {
        let mut socket = self
            .socket
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        write_frame(&mut *socket, message, body)
    }

    /// Send a frame with an empty body.
    pub fn send_message(&self, message: &Message) -> io::Result<()> {
        self.send(message, b"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frames_roundtrip_through_a_stream() {
        let message = Message::Hello {
            client_id: "c1".into(),
            version: "0.1.0".into(),
        };
        let mut buffer = Vec::new();
        write_frame(&mut buffer, &message, b"").unwrap();
        write_frame(
            &mut buffer,
            &Message::Data {
                session_id: "s1".into(),
                seq: 3,
                len: 4,
            },
            b"\x00ab\n",
        )
        .unwrap();

        let mut cursor = Cursor::new(buffer);
        let (first, body) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(first, message);
        assert!(body.is_empty());

        let (second, body) = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(
            second,
            Message::Data {
                session_id: "s1".into(),
                seq: 3,
                len: 4
            }
        );
        assert_eq!(body, b"\x00ab\n");

        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn clean_eof_is_reported_as_none() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn partial_length_prefix_is_an_error() {
        let mut cursor = Cursor::new(vec![0u8, 0, 0]);
        let err = read_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn truncated_payload_is_an_error() {
        let mut frame = Vec::new();
        write_frame(
            &mut frame,
            &Message::List {
                client_id: "c".into(),
            },
            b"",
        )
        .unwrap();
        let cut = frame.len() - 1;
        frame.truncate(cut);
        let mut cursor = Cursor::new(frame);
        assert_eq!(
            read_frame(&mut cursor).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn oversized_length_prefix_is_rejected_before_allocating() {
        let mut buffer = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
        buffer.extend_from_slice(b"{}");
        let mut cursor = Cursor::new(buffer);
        let err = read_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn malformed_header_is_an_invalid_data_error() {
        let header = b"not json";
        let mut buffer = ((header.len()) as u32).to_be_bytes().to_vec();
        buffer.extend_from_slice(header);
        let mut cursor = Cursor::new(buffer);
        assert_eq!(
            read_frame(&mut cursor).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn connection_out_writes_readable_frames() {
        let (left, right) = UnixStream::pair().unwrap();
        let out = ConnectionOut::new(left);
        out.send_message(&Message::Reply {
            id: 1,
            ok: true,
            detail: None,
        })
        .unwrap();
        let mut reader = right;
        let (message, body) = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(
            message,
            Message::Reply {
                id: 1,
                ok: true,
                detail: None
            }
        );
        assert!(body.is_empty());
    }

    #[test]
    fn connection_out_reports_a_closed_peer() {
        let (left, right) = UnixStream::pair().unwrap();
        drop(right);
        let out = ConnectionOut::new(left);
        let err = out
            .send_message(&Message::List {
                client_id: "c".into(),
            })
            .unwrap_err();
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
            ),
            "unexpected error: {err}"
        );
    }
}

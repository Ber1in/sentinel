//! Wire framing between the client and the remote helper.
//!
//! `u32 big-endian length` + JSON header + optional binary body. Binary bodies carry
//! terminal bytes so they never pay base64 overhead.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Largest frame we accept. A terminal burst is far smaller; this simply bounds memory.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame too large: {0} bytes")]
    TooLarge(usize),
    #[error("truncated frame")]
    Truncated,
    #[error("invalid header: {0}")]
    Header(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    Hello {
        client_id: String,
        version: String,
    },
    List {
        client_id: String,
    },
    Create {
        client_id: String,
        session_id: String,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    Attach {
        client_id: String,
        session_id: String,
    },
    Detach {
        client_id: String,
        session_id: String,
    },
    Resize {
        client_id: String,
        session_id: String,
        cols: u16,
        rows: u16,
    },
    Destroy {
        client_id: String,
        session_id: String,
    },
    /// Terminal bytes for a session (binary body carries the payload).
    Data {
        session_id: String,
        seq: u64,
        len: u32,
    },
    /// Reply to any request.
    Reply {
        id: u64,
        ok: bool,
        detail: Option<String>,
    },
    Event {
        session_id: String,
        what: String,
        detail: Option<String>,
    },
}

/// Serialize one frame.
pub fn encode(message: &Message, body: &[u8]) -> Vec<u8> {
    let header = serde_json::to_vec(message).expect("message is serializable");
    let mut frame = Vec::with_capacity(4 + header.len() + body.len());
    frame.extend_from_slice(&((header.len() + body.len()) as u32).to_be_bytes());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(body);
    frame
}

/// Parse one frame; `header_len` is returned so callers can split header from body.
pub fn decode(frame: &[u8]) -> Result<(Message, Vec<u8>), FrameError> {
    if frame.len() < 4 {
        return Err(FrameError::Truncated);
    }
    let total = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    if total > MAX_FRAME {
        return Err(FrameError::TooLarge(total));
    }
    if frame.len() < 4 + total {
        return Err(FrameError::Truncated);
    }
    let payload = &frame[4..4 + total];

    // The header is JSON; the body (if any) follows it. Try progressively shorter
    // prefixes so a binary body can never be mistaken for JSON.
    for split in (1..=payload.len()).rev() {
        if let Ok(message) = serde_json::from_slice::<Message>(&payload[..split]) {
            return Ok((message, payload[split..].to_vec()));
        }
        if payload.len() - split > 8192 {
            break;
        }
    }
    Err(FrameError::Header("no valid json header".into()))
}

/// Total length of a frame starting at `buffer`, or `None` when incomplete.
pub fn frame_len(buffer: &[u8]) -> Option<usize> {
    if buffer.len() < 4 {
        return None;
    }
    let total = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
    if total > MAX_FRAME {
        return None;
    }
    Some(4 + total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_roundtrips_without_body() {
        let message = Message::Hello {
            client_id: "c1".into(),
            version: "0.1.0".into(),
        };
        let frame = encode(&message, b"");
        let (decoded, body) = decode(&frame).unwrap();
        assert_eq!(decoded, message);
        assert!(body.is_empty());
    }

    #[test]
    fn data_frames_carry_binary_bodies() {
        let message = Message::Data {
            session_id: "s1".into(),
            seq: 7,
            len: 5,
        };
        let frame = encode(&message, b"\x1b[32mhi");
        let (decoded, body) = decode(&frame).unwrap();
        assert_eq!(decoded, message);
        assert_eq!(body, b"\x1b[32mhi");
    }

    #[test]
    fn every_control_message_roundtrips() {
        let messages = vec![
            Message::List {
                client_id: "c".into(),
            },
            Message::Create {
                client_id: "c".into(),
                session_id: "s".into(),
                cwd: Some("/tmp".into()),
                cols: 80,
                rows: 24,
            },
            Message::Attach {
                client_id: "c".into(),
                session_id: "s".into(),
            },
            Message::Detach {
                client_id: "c".into(),
                session_id: "s".into(),
            },
            Message::Resize {
                client_id: "c".into(),
                session_id: "s".into(),
                cols: 100,
                rows: 40,
            },
            Message::Destroy {
                client_id: "c".into(),
                session_id: "s".into(),
            },
            Message::Reply {
                id: 1,
                ok: true,
                detail: None,
            },
            Message::Event {
                session_id: "s".into(),
                what: "exit".into(),
                detail: Some("0".into()),
            },
        ];
        for message in messages {
            let frame = encode(&message, b"");
            assert_eq!(decode(&frame).unwrap().0, message);
        }
    }

    #[test]
    fn truncated_frames_are_rejected() {
        assert!(matches!(decode(&[0, 0]), Err(FrameError::Truncated)));
        let frame = encode(
            &Message::List {
                client_id: "c".into(),
            },
            b"",
        );
        assert!(matches!(
            decode(&frame[..frame.len() - 2]),
            Err(FrameError::Truncated)
        ));
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut frame = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(b"{}");
        assert!(matches!(decode(&frame), Err(FrameError::TooLarge(_))));
    }

    #[test]
    fn frame_len_reports_incomplete_buffers() {
        let frame = encode(
            &Message::List {
                client_id: "c".into(),
            },
            b"",
        );
        assert_eq!(frame_len(&frame[..2]), None);
        assert_eq!(frame_len(&frame), Some(frame.len()));
    }
}

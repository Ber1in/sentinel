//! Control-plane messages and the binary data-frame codec.
//!
//! Two transports share one WebSocket connection:
//! - control plane: JSON text frames carrying [`ClientMessage`] / [`ServerMessage`]
//! - data plane: binary frames carrying terminal bytes, see [`encode_data_frame`]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::session::{AuthMethod, HostSpec, ProviderKind, SessionState, SessionSummary};

/// Upper bound for one control message; larger frames are rejected by the daemon.
pub const MAX_CONTROL_BYTES: usize = 1024 * 1024;

/// Byte length of a binary data frame header.
pub const DATA_FRAME_HEADER_LEN: usize = 1 + 4 + 8;

/// Which direction a binary frame flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// Daemon -> GUI terminal output.
    Output,
    /// GUI -> daemon terminal input.
    Input,
}

impl FrameKind {
    fn as_u8(self) -> u8 {
        match self {
            FrameKind::Output => 1,
            FrameKind::Input => 2,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(FrameKind::Output),
            2 => Some(FrameKind::Input),
            _ => None,
        }
    }
}

/// Serialize one terminal data frame: `[kind][session:u32 LE][seq:u64 LE][payload]`.
pub fn encode_data_frame(kind: FrameKind, session: u32, seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(DATA_FRAME_HEADER_LEN + payload.len());
    frame.push(kind.as_u8());
    frame.extend_from_slice(&session.to_le_bytes());
    frame.extend_from_slice(&seq.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Parse one terminal data frame, returning `None` for truncated or unknown frames.
pub fn decode_data_frame(frame: &[u8]) -> Option<(FrameKind, u32, u64, &[u8])> {
    if frame.len() < DATA_FRAME_HEADER_LEN {
        return None;
    }
    let kind = FrameKind::from_u8(frame[0])?;
    let session = u32::from_le_bytes(frame[1..5].try_into().ok()?);
    let seq = u64::from_le_bytes(frame[5..13].try_into().ok()?);
    Some((kind, session, seq, &frame[DATA_FRAME_HEADER_LEN..]))
}

/// Message sent by the GUI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
        token: String,
        client_version: String,
    },
    Request {
        id: u32,
        op: ClientOp,
    },
}

/// Operations the GUI can request from the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientOp {
    ListSessions,
    /// Persisted hosts, newest first.
    ListHosts,
    /// Store a secret in the OS credential store under `id`.
    SetSecret {
        id: String,
        secret: String,
    },
    /// Create or update a saved host, including its display name and group.
    UpdateHost {
        host: HostSpec,
        name: String,
        group_name: Option<String>,
    },
    /// Remove a saved host and detach any live sessions belonging to it.
    DeleteHost {
        host_id: String,
    },
    /// Forget a stored secret (used when the authentication method changes).
    DeleteSecret {
        id: String,
    },
    /// Copy a saved host (including its secret) under a new id.
    DuplicateHost {
        host_id: String,
        new_id: String,
    },
    /// Replace the persisted tab/split layout.
    SaveLayout {
        tabs: Vec<TabLayout>,
        panes: Vec<PaneLayout>,
    },
    LoadLayout,
    CreateSession {
        host: HostSpec,
        pane_id: String,
        cwd: Option<String>,
    },
    Attach {
        session_id: String,
    },
    Detach {
        session_id: String,
    },
    /// Destroy the remote session as well as the local one.
    Destroy {
        session_id: String,
    },
    Resize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    /// Ask for a snapshot replay starting at `from_seq` (used after a dropped frame).
    Resync {
        session_id: String,
        from_seq: u64,
    },
    /// Graceful daemon restart, detaching remote sessions without killing them.
    RestartDaemon,
}

/// Message sent by the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerMessage {
    Hello { info: crate::ServerInfo },
    Reply { id: u32, result: OpResult },
    Event { event: ServerEvent },
}

/// Result payload of one [`ClientOp`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "result", rename_all = "snake_case")]
#[ts(export)]
pub enum OpResult {
    Ok,
    Sessions {
        sessions: Vec<SessionSummary>,
    },
    Hosts {
        hosts: Vec<HostSummary>,
    },
    Layout {
        tabs: Vec<TabLayout>,
        panes: Vec<PaneLayout>,
    },
    Session {
        session_id: String,
        /// Numeric id used in binary data frames.
        numeric: u32,
        provider: ProviderKind,
        /// True when the remote side had to be recreated (process is gone).
        recreated: bool,
    },
    Error {
        error: ProtocolError,
    },
}

/// Persisted host as shown in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HostSummary {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub group_name: Option<String>,
    pub last_used_at: i64,
}

/// One tab in the persisted layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TabLayout {
    pub id: String,
    pub title: String,
    pub sort_order: i64,
}

/// One pane in the persisted split tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PaneLayout {
    pub id: String,
    pub tab_id: String,
    pub parent_id: Option<String>,
    /// `horizontal` or `vertical`.
    pub direction: Option<String>,
    pub ratio: f64,
    pub host_id: Option<String>,
    pub session_id: Option<String>,
    pub sort_order: i64,
}

/// Structured, machine-readable error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<String>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(
        code: ErrorCode,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            detail: Some(detail.into()),
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}

/// Error classes the GUI renders differently; never derived from message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    /// Wrong password / rejected key. Must not be retried.
    AuthFailed,
    /// Known host key changed. Must not be retried.
    HostKeyMismatch,
    /// Transport-level failure; retryable.
    Network,
    /// Operation or handshake timed out; retryable.
    Timeout,
    ProtocolVersion,
    NotFound,
    InvalidRequest,
    TmuxMissing,
    /// The remote helper is not installed.
    HelperMissing,
    /// The remote helper exists but speaks an incompatible version.
    HelperIncompatible,
    /// The uploaded helper failed its checksum.
    HelperChecksum,
    Internal,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ErrorCode::AuthFailed => "auth_failed",
            ErrorCode::HostKeyMismatch => "host_key_mismatch",
            ErrorCode::Network => "network",
            ErrorCode::Timeout => "timeout",
            ErrorCode::ProtocolVersion => "protocol_version",
            ErrorCode::NotFound => "not_found",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::TmuxMissing => "tmux_missing",
            ErrorCode::HelperMissing => "helper_missing",
            ErrorCode::HelperIncompatible => "helper_incompatible",
            ErrorCode::HelperChecksum => "helper_checksum",
            ErrorCode::Internal => "internal",
        };
        f.write_str(text)
    }
}

/// Asynchronous notifications pushed by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "event", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerEvent {
    SessionState {
        session_id: String,
        state: SessionState,
        detail: Option<String>,
    },
    SessionCreated {
        session_id: String,
        provider: ProviderKind,
        recreated: bool,
        cwd: Option<String>,
    },
    SessionDestroyed {
        session_id: String,
    },
    DaemonShuttingDown {
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;

    fn host() -> HostSpec {
        HostSpec {
            host_id: "h1".into(),
            hostname: "10.0.0.5".into(),
            port: 22,
            username: "root".into(),
            auth: crate::session::AuthMethod::Agent,
        }
    }

    #[test]
    fn client_hello_roundtrips() {
        let message = ClientMessage::Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            token: "t".into(),
            client_version: "0.1.0".into(),
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["type"], "hello");
        assert_eq!(
            serde_json::from_value::<ClientMessage>(json).unwrap(),
            message
        );
    }

    #[test]
    fn every_client_op_roundtrips() {
        let ops = vec![
            ClientOp::ListSessions,
            ClientOp::CreateSession {
                host: host(),
                pane_id: "p1".into(),
                cwd: Some("/var/log".into()),
            },
            ClientOp::Attach {
                session_id: "s1".into(),
            },
            ClientOp::Detach {
                session_id: "s1".into(),
            },
            ClientOp::Destroy {
                session_id: "s1".into(),
            },
            ClientOp::Resize {
                session_id: "s1".into(),
                cols: 120,
                rows: 40,
            },
            ClientOp::Resync {
                session_id: "s1".into(),
                from_seq: 7,
            },
            ClientOp::RestartDaemon,
        ];
        for op in ops {
            let message = ClientMessage::Request {
                id: 9,
                op: op.clone(),
            };
            let json = serde_json::to_value(&message).unwrap();
            assert_eq!(json["type"], "request");
            assert_eq!(
                serde_json::from_value::<ClientMessage>(json).unwrap(),
                message
            );
        }
    }

    #[test]
    fn server_messages_roundtrip() {
        let messages = vec![
            ServerMessage::Hello {
                info: crate::ServerInfo {
                    protocol_version: crate::PROTOCOL_VERSION,
                    daemon_version: "0.1.0".into(),
                    pid: 1,
                },
            },
            ServerMessage::Reply {
                id: 3,
                result: OpResult::Sessions { sessions: vec![] },
            },
            ServerMessage::Event {
                event: ServerEvent::SessionState {
                    session_id: "s1".into(),
                    state: SessionState::Reconnecting,
                    detail: Some("link lost".into()),
                },
            },
            ServerMessage::Event {
                event: ServerEvent::DaemonShuttingDown {
                    reason: "upgrade".into(),
                },
            },
        ];
        for message in messages {
            let json = serde_json::to_value(&message).unwrap();
            assert_eq!(
                serde_json::from_value::<ServerMessage>(json).unwrap(),
                message
            );
        }
    }

    #[test]
    fn errors_and_results_roundtrip() {
        let reply = ServerMessage::Reply {
            id: 5,
            result: OpResult::Error {
                error: ProtocolError::with_detail(
                    ErrorCode::AuthFailed,
                    "bad credentials",
                    "publickey",
                ),
            },
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["result"]["error"]["code"], "auth_failed");
        assert_eq!(
            serde_json::from_value::<ServerMessage>(json).unwrap(),
            reply
        );
    }

    #[test]
    fn data_frame_roundtrips_payload() {
        let frame = encode_data_frame(FrameKind::Output, 7, 1_000_000, b"hello");
        assert_eq!(frame.len(), DATA_FRAME_HEADER_LEN + 5);
        let (kind, session, seq, payload) = decode_data_frame(&frame).unwrap();
        assert_eq!(kind, FrameKind::Output);
        assert_eq!(session, 7);
        assert_eq!(seq, 1_000_000);
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn decode_rejects_truncated_or_unknown_frames() {
        assert!(decode_data_frame(&[]).is_none());
        assert!(decode_data_frame(&[1, 0, 0]).is_none());
        let mut frame = encode_data_frame(FrameKind::Input, 1, 0, b"x");
        frame[0] = 99;
        assert!(decode_data_frame(&frame).is_none());
    }

    #[test]
    fn binary_frames_keep_empty_payload() {
        let frame = encode_data_frame(FrameKind::Output, 0, 0, b"");
        let (_, _, _, payload) = decode_data_frame(&frame).unwrap();
        assert!(payload.is_empty());
    }
}

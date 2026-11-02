//! Session-related wire types shared by the daemon and the GUI.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Lifecycle of a single terminal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionState {
    Connecting,
    Connected,
    Reconnecting,
    Detached,
    Lost,
    Failed,
}

impl SessionState {
    /// Terminal states never transition on their own.
    pub fn is_terminal(self) -> bool {
        matches!(self, SessionState::Failed)
    }

    /// Whether the state means a live link to the host.
    pub fn is_live(self) -> bool {
        matches!(self, SessionState::Connected)
    }
}

/// Which persistence mechanism owns the remote side of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProviderKind {
    /// Remote tmux session keeps the process alive.
    Tmux,
    /// No persistence: the shell dies with the connection.
    None,
}

/// How the client authenticates to a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "method", rename_all = "snake_case")]
#[ts(export)]
pub enum AuthMethod {
    Password {
        credential_id: String,
    },
    Key {
        key_path: String,
        credential_id: Option<String>,
    },
    Agent,
    KeyboardInteractive {
        credential_id: Option<String>,
    },
}

/// Everything needed to open a connection to one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HostSpec {
    pub host_id: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
}

/// Lightweight view of a session, used by the sidebar and session lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    pub session_id: String,
    pub host_id: String,
    pub title: String,
    pub state: SessionState,
    pub provider: ProviderKind,
    pub cwd: Option<String>,
    /// Unix epoch milliseconds of the last activity seen by the daemon.
    pub last_active_at: i64,
}

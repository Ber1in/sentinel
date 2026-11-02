//! Remote persistence providers.
//!
//! A provider owns the remote side of a pane: it decides whether the shell survives an
//! SSH disconnect, how a session is reattached, and how its visible history is recovered.
//! The daemon talks to providers through [`RemoteExec`], so the same logic runs against
//! a fake executor in tests and a real SSH channel in production.

use async_trait::async_trait;
use protocol::ProviderKind;

pub mod none;
pub mod tmux;

pub use none::NoPersistence;
pub use tmux::{TmuxOptions, TmuxPersistence};

/// Outcome of one command executed on the remote host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn failed(exit_code: i32, stderr: impl Into<String>) -> Self {
        Self {
            exit_code,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Executes a shell command on the remote host.
#[async_trait]
pub trait RemoteExec: Send + Sync {
    async fn run(&self, command: &str) -> Result<CommandOutput, PersistenceError>;
}

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("tmux is not installed on the remote host")]
    TmuxMissing,
    #[error("remote command failed: {0}")]
    CommandFailed(String),
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("malformed provider output: {0}")]
    BadOutput(String),
}

impl PersistenceError {
    pub fn code(&self) -> protocol::ErrorCode {
        match self {
            PersistenceError::TmuxMissing => protocol::ErrorCode::TmuxMissing,
            PersistenceError::CommandFailed(_) | PersistenceError::BadOutput(_) => {
                protocol::ErrorCode::Internal
            }
            PersistenceError::Transport(_) => protocol::ErrorCode::Network,
        }
    }
}

/// Where a pane's remote state came from when it was bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// An existing remote session was reattached; its processes are still alive.
    Attached,
    /// No remote session existed, so a fresh one was created.
    Recreated,
    /// The provider cannot persist anything; the shell dies with the connection.
    Unpersisted,
}

/// The contract every provider implements. Implementations must be cheap to clone/share
/// and must not hold the SSH connection themselves — they receive an executor.
#[async_trait]
pub trait PersistenceProvider: Send + Sync {
    /// Provider kind reported over the wire.
    fn kind(&self) -> ProviderKind;

    /// Stable remote session name for a pane. `namespace` identifies the client
    /// installation so two clients never attach to each other's shells. Must be
    /// deterministic: reconnects rely on regenerating the same name.
    fn session_name(&self, namespace: &str, host_id: &str, pane_id: &str) -> String;

    /// Program+arguments used to open the interactive pane. Running it attaches to an
    /// existing session or creates one.
    fn attach_command(&self, session_name: &str, cwd: Option<&str>) -> String;

    /// Bind to an existing session or create a new one, reporting which happened.
    async fn ensure(
        &self,
        exec: &dyn RemoteExec,
        session_name: &str,
        cwd: Option<&str>,
    ) -> Result<EnsureOutcome, PersistenceError>;

    /// Visible history for replay, already colourised when the provider supports it.
    async fn snapshot(
        &self,
        exec: &dyn RemoteExec,
        session_name: &str,
        lines: usize,
    ) -> Result<String, PersistenceError>;

    /// Destroy the remote session (explicit user action only).
    async fn destroy(
        &self,
        exec: &dyn RemoteExec,
        session_name: &str,
    ) -> Result<(), PersistenceError>;
}

/// Shell quoting for a single POSIX argument.
pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_leaves_safe_words_alone() {
        assert_eq!(shell_quote("/var/log"), "/var/log");
        assert_eq!(shell_quote("st_host_pane"), "st_host_pane");
    }

    #[test]
    fn shell_quote_escapes_spaces_and_quotes() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("it's"), r#"'it'\''s'"#);
        assert_eq!(shell_quote(""), "''");
    }
}

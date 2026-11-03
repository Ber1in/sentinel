//! Error type shared by the [`RemoteFs`](crate::RemoteFs) implementation and the
//! transfer helpers.

use thiserror::Error;

use crate::path::PathError;

#[derive(Debug, Error)]
pub enum SftpError {
    /// A path was rejected before anything was sent to the server.
    #[error(transparent)]
    Path(#[from] PathError),

    /// Opening the subsystem channel failed.
    #[error(transparent)]
    Ssh(#[from] ssh::SshError),

    /// The server rejected a request, timed out, or broke the protocol.
    #[error("sftp error: {0}")]
    Protocol(#[from] russh_sftp::client::error::Error),

    /// Local filesystem I/O, or I/O on the SFTP stream surfaced through a file
    /// handle. The distinction does not matter to the panel, which only shows
    /// the message.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The caller's [`CancelToken`](crate::CancelToken) was tripped. Any partial
    /// file has been removed.
    #[error("transfer cancelled")]
    Cancelled,

    /// `chmod` only accepts the low twelve bits (`0o7777`).
    #[error("invalid permission mode {0:#o}")]
    InvalidMode(u32),
}

impl SftpError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

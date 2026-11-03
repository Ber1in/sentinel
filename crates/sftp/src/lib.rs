//! SFTP client used by the file panel. Owned by the daemon, not the GUI.
//!
//! The client wraps [`russh_sftp::client::SftpSession`], which speaks SFTP v3
//! over any `AsyncRead + AsyncWrite` stream. We give it the stream form of the
//! subsystem channel opened by [`ssh::Connection::open_subsystem`], so SFTP
//! shares the SSH connection with the interactive shell.
//!
//! Two rules shape the API:
//!
//! * Every remote path is a [`RemotePath`]; it can only be built by parsing an
//!   absolute path that contains no `..`, so path traversal is rejected at the
//!   type level rather than at each call site.
//! * Transfers never write the destination directly. They write
//!   `<name>.part-<task>` and rename it into place when the bytes are on disk,
//!   which keeps a half-transferred file out of the user's listing. Both
//!   directions accept a [`CancelToken`] and a progress callback.

mod cancel;
mod client;
mod entry;
mod error;
mod path;
mod transfer;

pub use cancel::CancelToken;
pub use client::SftpFs;
pub use entry::Entry;
pub use error::SftpError;
pub use path::{PathError, RemotePath};

use async_trait::async_trait;

/// The remote filesystem operations the file panel needs.
///
/// Paths are validated by [`RemotePath`] before they reach an implementation.
#[async_trait]
pub trait RemoteFs: Send + Sync {
    /// Entries in `dir`, sorted by name. `.` and `..` are omitted, as are
    /// entries whose names are not a single safe path component.
    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, SftpError>;

    /// Create one directory. The parent must already exist.
    async fn mkdir(&self, path: &RemotePath) -> Result<(), SftpError>;

    /// Rename or move a file or directory.
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), SftpError>;

    /// Remove a file, a symlink, or a directory tree.
    async fn remove(&self, path: &RemotePath) -> Result<(), SftpError>;

    /// Change permission bits. Only the low twelve bits are accepted.
    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), SftpError>;

    /// Metadata for `path` itself: symlinks are not followed.
    async fn stat(&self, path: &RemotePath) -> Result<Entry, SftpError>;
}

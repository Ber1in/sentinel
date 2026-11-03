//! [`SftpFs`]: the [`RemoteFs`] implementation backed by a real SFTP subsystem.

use async_trait::async_trait;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};

use crate::entry::Entry;
use crate::error::SftpError;
use crate::path::{validate_component, RemotePath};
use crate::RemoteFs;

/// An SFTP session over one SSH channel.
///
/// All filesystem calls are serialised by the underlying session, so a single
/// value is shared between tasks behind an `Arc` rather than opened per request.
pub struct SftpFs {
    pub(crate) session: SftpSession,
}

impl SftpFs {
    /// Open the `sftp` subsystem on an authenticated connection.
    pub async fn connect(connection: &ssh::Connection) -> Result<Self, SftpError> {
        let channel = connection.open_subsystem("sftp").await?;
        let session = SftpSession::new(channel.into_stream()).await?;
        Ok(Self { session })
    }

    /// Wrap an already-initialised session (tests, alternative transports).
    pub fn from_session(session: SftpSession) -> Self {
        Self { session }
    }

    pub fn session(&self) -> &SftpSession {
        &self.session
    }

    /// The session's default directory, normally the login user's home.
    pub async fn home(&self) -> Result<RemotePath, SftpError> {
        let home = self.session.canonicalize(".").await?;
        Ok(RemotePath::parse(home)?)
    }

    /// Resolve `..`, `.` and symlinks on the server for an already-validated
    /// path. The result is validated again so it can be fed back into the panel.
    pub async fn canonicalize(&self, path: &RemotePath) -> Result<RemotePath, SftpError> {
        let resolved = self.session.canonicalize(path.as_str()).await?;
        Ok(RemotePath::parse(resolved)?)
    }

    pub(crate) async fn open_for_write(
        &self,
        path: &RemotePath,
    ) -> Result<russh_sftp::client::fs::File, SftpError> {
        Ok(self
            .session
            .open_with_flags(
                path.as_str(),
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            )
            .await?)
    }

    pub(crate) async fn open_for_read(
        &self,
        path: &RemotePath,
    ) -> Result<russh_sftp::client::fs::File, SftpError> {
        Ok(self.session.open(path.as_str()).await?)
    }
}

#[async_trait]
impl RemoteFs for SftpFs {
    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, SftpError> {
        let mut entries = Vec::new();
        for entry in self.session.read_dir(dir.as_str()).await? {
            let name = entry.file_name();
            if let Err(error) = validate_component(&name) {
                tracing::warn!(dir = %dir, name = %name, %error, "skipping unsafe directory entry");
                continue;
            }
            entries.push(Entry::from_attrs(name, &entry.metadata()));
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    async fn mkdir(&self, path: &RemotePath) -> Result<(), SftpError> {
        Ok(self.session.create_dir(path.as_str()).await?)
    }

    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), SftpError> {
        if from == to {
            return Ok(());
        }
        Ok(self.session.rename(from.as_str(), to.as_str()).await?)
    }

    async fn remove(&self, path: &RemotePath) -> Result<(), SftpError> {
        // Post-order walk with an explicit stack: the trait is async, so
        // recursion would need boxing, and a hostile tree could exhaust the
        // stack anyway.
        let root = self.stat(path).await?;
        if !root.is_dir || root.is_symlink {
            return Ok(self.session.remove_file(path.as_str()).await?);
        }

        let mut stack = vec![(path.clone(), false)];
        while let Some((current, visited)) = stack.pop() {
            if visited {
                self.session.remove_dir(current.as_str()).await?;
                continue;
            }
            stack.push((current.clone(), true));
            for entry in self.list(&current).await? {
                let child = current.join(&entry.name)?;
                if entry.is_dir && !entry.is_symlink {
                    stack.push((child, false));
                } else {
                    // Unlinking a symlink removes the link, not its target.
                    self.session.remove_file(child.as_str()).await?;
                }
            }
        }
        Ok(())
    }

    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), SftpError> {
        if mode & !0o7777 != 0 {
            return Err(SftpError::InvalidMode(mode));
        }
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(mode);
        Ok(self.session.set_metadata(path.as_str(), attrs).await?)
    }

    async fn stat(&self, path: &RemotePath) -> Result<Entry, SftpError> {
        let attrs = self.session.symlink_metadata(path.as_str()).await?;
        let name = if path.is_root() { "/" } else { path.name() };
        Ok(Entry::from_attrs(name, &attrs))
    }
}

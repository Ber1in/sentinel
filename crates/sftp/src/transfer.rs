//! Upload and download with the `.part-<task>` rename strategy.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::cancel::CancelToken;
use crate::client::SftpFs;
use crate::error::SftpError;
use crate::path::{validate_component, RemotePath};

/// Bytes moved per request in both directions.
///
/// One request per chunk keeps progress live, keeps memory bounded, and gives
/// the cancellation token a chance to fire between chunks.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// `(bytes done, total bytes)`. `total` is 0 when the server did not report a
/// size; it never shrinks below `done`.
pub type ProgressSink<'a> = &'a mut (dyn FnMut(u64, u64) + Send);

impl SftpFs {
    /// Upload `local` to `remote` through `<name>.part-<task>`.
    ///
    /// The part file is renamed into place only after every byte has been
    /// written and `fsync`ed. If the destination exists it is removed just
    /// before the rename, because SFTP v3 `RENAME` refuses to overwrite on
    /// OpenSSH. On error or cancellation the part file is removed and the
    /// destination is left untouched.
    ///
    /// Returns the number of bytes written.
    pub async fn upload(
        &self,
        local: &Path,
        remote: &RemotePath,
        task: &str,
        cancel: &CancelToken,
        progress: ProgressSink<'_>,
    ) -> Result<u64, SftpError> {
        let part = remote.part(task)?;
        match self
            .stage_upload(local, remote, &part, cancel, progress)
            .await
        {
            Ok(written) => Ok(written),
            Err(error) => {
                let _ = self.session.remove_file(part.as_str()).await;
                Err(error)
            }
        }
    }

    /// Download `remote` to `local` through `<local name>.part-<task>`.
    ///
    /// The local part file is renamed into place after it has been flushed and
    /// synced. If the destination exists it is removed first so the rename
    /// works on Windows too. On error or cancellation the part file is removed.
    ///
    /// Returns the number of bytes written.
    pub async fn download(
        &self,
        remote: &RemotePath,
        local: &Path,
        task: &str,
        cancel: &CancelToken,
        progress: ProgressSink<'_>,
    ) -> Result<u64, SftpError> {
        let part = local_part(local, task)?;
        match self
            .stage_download(remote, local, &part, cancel, progress)
            .await
        {
            Ok(written) => Ok(written),
            Err(error) => {
                let _ = tokio::fs::remove_file(&part).await;
                Err(error)
            }
        }
    }

    async fn stage_upload(
        &self,
        local: &Path,
        remote: &RemotePath,
        part: &RemotePath,
        cancel: &CancelToken,
        progress: ProgressSink<'_>,
    ) -> Result<u64, SftpError> {
        let total = tokio::fs::metadata(local).await?.len();
        if cancel.is_cancelled() {
            return Err(SftpError::Cancelled);
        }

        let mut source = tokio::fs::File::open(local).await?;
        let mut target = self.open_for_write(part).await?;
        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut done = 0u64;

        loop {
            if cancel.is_cancelled() {
                return Err(SftpError::Cancelled);
            }
            let read = tokio::io::AsyncReadExt::read(&mut source, &mut buffer).await?;
            if read == 0 {
                break;
            }
            tokio::io::AsyncWriteExt::write_all(&mut target, &buffer[..read]).await?;
            done += read as u64;
            progress(done, total.max(done));
        }

        target.sync_all().await?;
        target.close().await?;

        // Never publish a file after the caller gave up.
        if cancel.is_cancelled() {
            return Err(SftpError::Cancelled);
        }

        // SFTP v3 rename does not overwrite on OpenSSH. Removing the destination
        // first is the only portable option russh-sftp exposes.
        if self.session.try_exists(remote.as_str()).await? {
            self.session.remove_file(remote.as_str()).await?;
        }
        self.session.rename(part.as_str(), remote.as_str()).await?;
        progress(done, total.max(done));
        Ok(done)
    }

    async fn stage_download(
        &self,
        remote: &RemotePath,
        local: &Path,
        part: &Path,
        cancel: &CancelToken,
        progress: ProgressSink<'_>,
    ) -> Result<u64, SftpError> {
        if cancel.is_cancelled() {
            return Err(SftpError::Cancelled);
        }

        let total = self
            .session
            .metadata(remote.as_str())
            .await?
            .size
            .unwrap_or(0);
        let mut source = self.open_for_read(remote).await?;
        let mut target = tokio::fs::File::create(part).await?;
        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut done = 0u64;

        loop {
            if cancel.is_cancelled() {
                return Err(SftpError::Cancelled);
            }
            let read = tokio::io::AsyncReadExt::read(&mut source, &mut buffer).await?;
            if read == 0 {
                break;
            }
            tokio::io::AsyncWriteExt::write_all(&mut target, &buffer[..read]).await?;
            done += read as u64;
            progress(done, total.max(done));
        }

        tokio::io::AsyncWriteExt::flush(&mut target).await?;
        target.sync_all().await?;
        drop(target);

        if cancel.is_cancelled() {
            return Err(SftpError::Cancelled);
        }

        // `rename` is atomic on Unix but refuses to replace on Windows.
        match tokio::fs::remove_file(local).await {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        tokio::fs::rename(part, local).await?;
        progress(done, total.max(done));
        Ok(done)
    }
}

/// `<local>.part-<task>`, refusing directories and unsafe task ids.
fn local_part(local: &Path, task: &str) -> Result<PathBuf, SftpError> {
    validate_component(task)?;
    let name = local.file_name().ok_or_else(|| {
        crate::path::PathError::InvalidName(format!("{}: not a file name", local.display()))
    })?;
    let mut part = OsString::from(name);
    part.push(format!(".part-{task}"));
    Ok(local.with_file_name(part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_files_keep_the_local_directory() {
        let part = local_part(Path::new("/tmp/report.pdf"), "abc").unwrap();
        assert_eq!(part, PathBuf::from("/tmp/report.pdf.part-abc"));
    }

    #[test]
    fn part_files_reject_unsafe_task_ids() {
        for bad in ["", ".", "..", "a/b", "x\0y"] {
            assert!(
                local_part(Path::new("/tmp/report.pdf"), bad).is_err(),
                "task {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn part_files_require_a_file_name() {
        assert!(local_part(Path::new("/"), "abc").is_err());
    }
}

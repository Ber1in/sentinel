//! Single-instance enforcement through an advisory file lock.

use std::fs::{File, OpenOptions};
use std::path::Path;

use anyhow::{Context, Result};
use fs2::FileExt;

/// Holds the exclusive lock for the lifetime of the daemon process.
#[derive(Debug)]
pub struct SingleInstance {
    _file: File,
}

impl SingleInstance {
    /// Try to become the only daemon for this data directory.
    ///
    /// Returns `Ok(None)` when another process already holds the lock.
    pub fn acquire(path: &Path) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open lock file {}", path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_rejected_until_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");

        let first = SingleInstance::acquire(&path).unwrap();
        assert!(first.is_some(), "first instance must acquire the lock");
        assert!(
            SingleInstance::acquire(&path).unwrap().is_none(),
            "second instance must be rejected"
        );

        drop(first);
        assert!(
            SingleInstance::acquire(&path).unwrap().is_some(),
            "lock must be released when the holder exits"
        );
    }
}

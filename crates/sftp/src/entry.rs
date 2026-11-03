//! Directory entries returned by [`RemoteFs::list`](crate::RemoteFs::list) and
//! [`RemoteFs::stat`](crate::RemoteFs::stat).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh_sftp::protocol::FileAttributes;

/// One remote file or directory, flattened into the shape the file panel needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Base name, never a path.
    pub name: String,
    /// Size in bytes. Zero when the server did not report one.
    pub size: u64,
    /// Modification time. SFTP v3 only has whole seconds; `None` when the
    /// server omitted the attribute.
    pub mtime: Option<SystemTime>,
    /// Unix mode bits, including the file type bits (for example `0o100644`).
    pub permissions: Option<u32>,
    pub is_dir: bool,
    pub is_symlink: bool,
}

impl Entry {
    /// Parse one SFTP v3 `SSH_FXP_NAME`/`SSH_FXP_ATTRS` entry.
    ///
    /// Servers are allowed to omit individual attributes, so a missing size is
    /// reported as zero and a missing mode as `None` instead of failing the
    /// whole listing. The type bits of `permissions` decide `is_dir` and
    /// `is_symlink`; unknown types are treated as regular files.
    pub fn from_attrs(name: impl Into<String>, attrs: &FileAttributes) -> Self {
        let file_type = attrs.file_type();
        Self {
            name: name.into(),
            size: attrs.size.unwrap_or(0),
            mtime: attrs
                .mtime
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds as u64)),
            permissions: attrs.permissions,
            is_dir: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
        }
    }

    /// Not a directory and not a symbolic link. Special files (fifos, sockets,
    /// devices) are reported as files because SFTP v3 does not expose them.
    pub fn is_file(&self) -> bool {
        !self.is_dir && !self.is_symlink
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(mode: u32) -> FileAttributes {
        FileAttributes {
            size: Some(1234),
            permissions: Some(mode),
            mtime: Some(1_700_000_000),
            ..FileAttributes::empty()
        }
    }

    #[test]
    fn parses_a_regular_file() {
        let entry = Entry::from_attrs("notes.txt", &attrs(0o100644));
        assert_eq!(entry.name, "notes.txt");
        assert_eq!(entry.size, 1234);
        assert_eq!(entry.permissions, Some(0o100644));
        assert_eq!(
            entry.mtime,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
        assert!(entry.is_file());
        assert!(!entry.is_dir);
        assert!(!entry.is_symlink);
    }

    #[test]
    fn parses_a_directory_and_a_symlink() {
        let dir = Entry::from_attrs("src", &attrs(0o040755));
        assert!(dir.is_dir);
        assert!(!dir.is_symlink);
        assert!(!dir.is_file());

        let link = Entry::from_attrs("current", &attrs(0o120777));
        assert!(link.is_symlink);
        assert!(!link.is_dir);
        assert!(!link.is_file());
    }

    #[test]
    fn missing_attributes_get_safe_defaults() {
        let entry = Entry::from_attrs("mystery", &FileAttributes::empty());
        assert_eq!(entry.size, 0);
        assert_eq!(entry.permissions, None);
        assert_eq!(entry.mtime, None);
        // Unknown type bits must not be reported as a directory or link.
        assert!(!entry.is_dir);
        assert!(!entry.is_symlink);
    }

    #[test]
    fn unix_epoch_is_representable() {
        let mut attrs = attrs(0o100600);
        attrs.mtime = Some(0);
        let entry = Entry::from_attrs("epoch", &attrs);
        assert_eq!(entry.mtime, Some(UNIX_EPOCH));
    }
}

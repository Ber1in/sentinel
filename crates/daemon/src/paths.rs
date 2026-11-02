//! Filesystem locations used by the daemon.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Resolved per-user daemon state directory.
#[derive(Debug, Clone)]
pub struct DaemonPaths {
    root: PathBuf,
}

impl DaemonPaths {
    /// Use `SENTINEL_DATA_DIR` when set (tests, portable installs), otherwise the
    /// platform's per-user data directory.
    pub fn resolve() -> Result<Self> {
        let root = match std::env::var_os("SENTINEL_DATA_DIR") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => platform_data_dir()?,
        };
        Ok(Self { root })
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// JSON handshake file: pid, port, token, protocol version.
    pub fn daemon_file(&self) -> PathBuf {
        self.root.join("daemon.json")
    }

    /// Advisory lock that enforces a single daemon per user.
    pub fn lock_file(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }

    /// Lock that keeps a single supervisor per user.
    pub fn supervisor_lock(&self) -> PathBuf {
        self.root.join("supervisor.lock")
    }

    pub fn scrollback_dir(&self) -> PathBuf {
        self.root.join("scrollback")
    }

    pub fn state_db(&self) -> PathBuf {
        self.root.join("state.sqlite3")
    }

    pub fn log_file(&self) -> PathBuf {
        self.root.join("daemon.log")
    }

    /// Create the root directory with owner-only permissions where the platform allows it.
    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("create data dir {}", self.root.display()))?;
        restrict_to_owner(&self.root)?;
        Ok(())
    }
}

fn platform_data_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
        Ok(PathBuf::from(base).join("sentinel"))
    }
    #[cfg(not(windows))]
    {
        if let Some(base) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(base).join("sentinel"));
        }
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(PathBuf::from(home).join(".local/share/sentinel"))
    }
}

/// Restrict a path to the current user. Windows relies on the per-user data directory's
/// inherited ACL; Unix gets an explicit mode.
pub fn restrict_to_owner(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
    Ok(())
}

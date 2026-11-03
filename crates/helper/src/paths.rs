//! Filesystem locations used by the helper daemon.
//!
//! Everything lives under `~/.local/share/sentinel` so the daemon and the
//! installer agree on one place. `XDG_RUNTIME_DIR` is deliberately ignored:
//! it is often unset for SSH exec sessions.
//!
//! Overrides (all optional, used by deployment and by tests):
//! - `SENTINEL_HELPER_SOCKET` – Unix socket path.
//! - `SENTINEL_HELPER_LOCK` – `flock` path (defaults to `helper.lock` next to the socket).
//! - `SENTINEL_HELPER_LOG` – log file written by the launcher (not by `serve` itself).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Socket file name inside the run directory.
pub const SOCKET_FILE: &str = "helper.sock";
/// Lock file name inside the run directory.
pub const LOCK_FILE: &str = "helper.lock";
/// Log file name inside the run directory.
#[allow(dead_code)] // used by the installer (task 3.x) and by `log_path`
pub const LOG_FILE: &str = "helper.log";

fn non_empty_env(key: &str) -> Option<OsString> {
    match std::env::var_os(key) {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// `$HOME`, falling back to the current directory when unset.
pub fn home_dir() -> PathBuf {
    non_empty_env("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.local/share/sentinel`.
pub fn state_dir() -> PathBuf {
    state_dir_in(&home_dir())
}

/// `$home/.local/share/sentinel` — pure variant used by tests.
pub fn state_dir_in(home: &Path) -> PathBuf {
    home.join(".").join("local").join("share").join("sentinel")
}

/// Default run directory: `~/.local/share/sentinel/run`.
pub fn default_run_dir() -> PathBuf {
    state_dir().join("run")
}

/// Unix socket the daemon listens on (`SENTINEL_HELPER_SOCKET` overrides).
pub fn socket_path() -> PathBuf {
    non_empty_env("SENTINEL_HELPER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_run_dir().join(SOCKET_FILE))
}

/// Single-instance lock file (`SENTINEL_HELPER_LOCK` overrides).
///
/// It defaults to `helper.lock` in the socket's directory so that overriding
/// the socket for a test run also relocates the lock.
pub fn lock_path() -> PathBuf {
    if let Some(path) = non_empty_env("SENTINEL_HELPER_LOCK") {
        return PathBuf::from(path);
    }
    socket_path()
        .parent()
        .map(|dir| dir.join(LOCK_FILE))
        .unwrap_or_else(|| default_run_dir().join(LOCK_FILE))
}

/// Log file the launcher redirects `serve` output to (`SENTINEL_HELPER_LOG` overrides).
///
/// `serve` itself only writes to stderr; the deploy step redirects it here.
#[allow(dead_code)]
pub fn log_path() -> PathBuf {
    non_empty_env("SENTINEL_HELPER_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_run_dir().join(LOG_FILE))
}

/// Create `dir` (and parents) with mode `0700`.
///
/// Returns an error when the directory exists but is not private enough,
/// mirroring design decision D2: socket access is the only authentication
/// boundary this helper has.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match fs::create_dir_all(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    let mode = fs::metadata(dir)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Restrict `path` to the current user (`0600`).
pub fn restrict_file(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_is_under_home() {
        let dir = state_dir_in(Path::new("/home/someone"));
        assert_eq!(
            dir,
            PathBuf::from("/home/someone/.local/share/sentinel")
        );
        assert_eq!(dir.join("run"), PathBuf::from("/home/someone/.local/share/sentinel/run"));
    }

    #[test]
    fn socket_and_lock_share_a_directory() {
        let socket = socket_path();
        let lock = lock_path();
        assert!(socket.ends_with(SOCKET_FILE));
        assert!(lock.ends_with(LOCK_FILE));
        assert_eq!(socket.parent(), lock.parent());
    }

    #[test]
    fn default_run_dir_ends_with_run() {
        assert!(default_run_dir().ends_with("run"));
    }

    #[test]
    fn ensure_private_dir_creates_0700_tree() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("a/b/c");
        ensure_private_dir(&nested).unwrap();
        let mode = fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        // Idempotent.
        ensure_private_dir(&nested).unwrap();
    }

    #[test]
    fn ensure_private_dir_tightens_permissive_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("loose");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn restrict_file_sets_0600() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("x");
        fs::write(&file, b"x").unwrap();
        restrict_file(&file).unwrap();
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

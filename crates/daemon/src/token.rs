//! Handshake token shared with the GUI through a user-only file.

use std::path::Path;

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::paths::restrict_to_owner;

/// Contents of `daemon.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonFile {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub protocol_version: u32,
    pub daemon_version: String,
    /// Unix epoch milliseconds.
    pub started_at: i64,
}

/// 256-bit hex token. Short-lived by nature: rewritten on every daemon start.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn write_daemon_file(path: &Path, contents: &DaemonFile) -> Result<()> {
    let json = serde_json::to_vec_pretty(contents)?;
    std::fs::write(path, json).with_context(|| format!("write daemon file {}", path.display()))?;
    restrict_to_owner(path)?;
    Ok(())
}

pub fn read_daemon_file(path: &Path) -> Result<DaemonFile> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read daemon file {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn daemon_file_roundtrips_and_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.json");
        let file = DaemonFile {
            pid: 123,
            port: 4567,
            token: generate_token(),
            protocol_version: protocol::PROTOCOL_VERSION,
            daemon_version: "0.1.0".into(),
            started_at: now_millis(),
        };
        write_daemon_file(&path, &file).unwrap();
        assert_eq!(read_daemon_file(&path).unwrap(), file);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must not be world readable");
        }
    }
}

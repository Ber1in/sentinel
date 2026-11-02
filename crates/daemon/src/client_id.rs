//! Stable per-installation client identity.
//!
//! The id is created once, stored next to the daemon state, and reused forever. It
//! namespaces remote sessions so two Windows installs never attach to each other's
//! shells. It is an identifier, not a credential: anyone with the same Unix account can
//! still read the remote socket or attach to tmux manually.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientIdentity {
    pub client_id: String,
    pub created_at: i64,
}

impl ClientIdentity {
    /// First eight hex characters, used as the session-name namespace.
    pub fn short(&self) -> String {
        self.client_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(8)
            .collect()
    }
}

/// Load the identity from `<dir>/client.json`, creating it on first run.
pub fn load_or_create(dir: &Path) -> Result<ClientIdentity> {
    let path = dir.join("client.json");
    if path.exists() {
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        match serde_json::from_str::<ClientIdentity>(&text) {
            Ok(identity) if !identity.client_id.is_empty() => return Ok(identity),
            _ => {
                // Keep the unreadable file: the old namespace may still own remote
                // sessions the user wants to reclaim.
                let backup = path.with_extension(format!("corrupt-{}", now_millis()));
                let _ = std::fs::rename(&path, backup);
            }
        }
    }

    let identity = ClientIdentity {
        client_id: uuid::Uuid::new_v4().to_string(),
        created_at: now_millis(),
    };
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, serde_json::to_vec_pretty(&identity)?)?;
    crate::paths::restrict_to_owner(&path)?;
    Ok(identity)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create(dir.path()).unwrap();
        let second = load_or_create(dir.path()).unwrap();
        assert_eq!(first, second, "the id must survive restarts");
        assert_eq!(first.client_id.len(), 36, "expected a uuid4 string");
    }

    #[test]
    fn short_namespace_is_eight_alphanumeric_characters() {
        let identity = ClientIdentity {
            client_id: "d1080750-26d7-4352-8464-bd5413a24d84".into(),
            created_at: 0,
        };
        assert_eq!(identity.short(), "d1080750");
    }

    #[test]
    fn a_corrupt_file_is_replaced_with_a_fresh_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("client.json"), b"not json").unwrap();
        let identity = load_or_create(dir.path()).unwrap();
        assert!(!identity.client_id.is_empty());
    }

    #[test]
    fn identity_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        load_or_create(dir.path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("client.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}

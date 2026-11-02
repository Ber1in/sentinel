//! Known-host tracking and server key verification.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use russh::keys::key::PublicKey;

use crate::error::SshError;

/// In-memory + on-disk record of accepted server keys.
#[derive(Debug, Clone, Default)]
pub struct KnownHosts {
    entries: HashMap<String, String>,
    path: Option<PathBuf>,
}

/// How a connection treats the server key.
#[derive(Debug, Clone)]
pub enum HostKeyPolicy {
    /// Accept any key (first-run flows before the user confirms).
    AcceptAny,
    /// Trust-on-first-use with a persisted store.
    TrustOnFirstUse(Arc<Mutex<KnownHosts>>),
}

impl KnownHosts {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut hosts = Self {
            entries: HashMap::new(),
            path: Some(path.clone()),
        };
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, fingerprint)) = line.split_once(' ') {
                    hosts
                        .entries
                        .insert(key.to_string(), fingerprint.to_string());
                }
            }
        }
        Ok(hosts)
    }

    pub fn known(&self, host: &str, port: u16) -> Option<&str> {
        self.entries
            .get(&entry_key(host, port))
            .map(|value| value.as_str())
    }

    /// Record or verify a fingerprint. Returns `Err` when it changed.
    pub fn verify(&mut self, host: &str, port: u16, fingerprint: &str) -> Result<(), SshError> {
        let key = entry_key(host, port);
        match self.entries.get(&key) {
            Some(known) if known != fingerprint => Err(SshError::HostKeyMismatch {
                host: format!("{host}:{port}"),
                expected: known.clone(),
                actual: fingerprint.to_string(),
            }),
            Some(_) => Ok(()),
            None => {
                self.entries.insert(key, fingerprint.to_string());
                self.save()
                    .map_err(|error| SshError::Internal(error.to_string()))?;
                Ok(())
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut lines: Vec<String> = self
            .entries
            .iter()
            .map(|(host, fingerprint)| format!("{host} {fingerprint}"))
            .collect();
        lines.sort();
        std::fs::write(path, lines.join("\n") + "\n")?;
        crate::hostkey::restrict_permissions(path)?;
        Ok(())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

fn entry_key(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

pub fn restrict_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Check a presented key against the policy. `host`/`port` identify the target.
pub fn check(
    policy: &HostKeyPolicy,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<(), SshError> {
    let fingerprint = key.fingerprint();
    match policy {
        HostKeyPolicy::AcceptAny => Ok(()),
        HostKeyPolicy::TrustOnFirstUse(store) => {
            let mut store = store
                .lock()
                .map_err(|_| SshError::Internal("known hosts lock poisoned".into()))?;
            store.verify(host, port, &fingerprint)?;
            store
                .save()
                .map_err(|error| SshError::Internal(error.to_string()))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_use_is_recorded_then_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosts = KnownHosts::at(dir.path().join("known_hosts")).unwrap();
        hosts.verify("10.0.0.5", 22, "SHA256:aaa").unwrap();
        assert_eq!(hosts.known("10.0.0.5", 22), Some("SHA256:aaa"));
        hosts.verify("10.0.0.5", 22, "SHA256:aaa").unwrap();
    }

    #[test]
    fn changed_fingerprint_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosts = KnownHosts::at(dir.path().join("known_hosts")).unwrap();
        hosts.verify("10.0.0.5", 22, "SHA256:aaa").unwrap();
        let error = hosts.verify("10.0.0.5", 22, "SHA256:bbb").unwrap_err();
        assert_eq!(error.code(), protocol::ErrorCode::HostKeyMismatch);
    }

    #[test]
    fn store_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        {
            let mut hosts = KnownHosts::at(&path).unwrap();
            hosts.verify("host", 2222, "SHA256:ccc").unwrap();
        }
        let reloaded = KnownHosts::at(&path).unwrap();
        assert_eq!(reloaded.known("host", 2222), Some("SHA256:ccc"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn ports_are_part_of_the_identity() {
        let mut hosts = KnownHosts::in_memory();
        hosts.verify("host", 22, "SHA256:a").unwrap();
        assert_eq!(hosts.known("host", 2222), None);
    }
}

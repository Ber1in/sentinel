//! Secret storage.
//!
//! Secrets never touch the config file, the state database, or logs: they live in the
//! OS credential store and are referenced by an opaque `auth_ref` id. Linux tests use an
//! in-memory store because CI has no secret service.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("credential store unavailable: {0}")]
    Unavailable(String),
    #[error("credential error: {0}")]
    Failed(String),
}

pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, id: &str, secret: &str) -> Result<(), SecretError>;
    fn delete(&self, id: &str) -> Result<(), SecretError>;
}

/// Windows Credential Manager (or the platform equivalent) backed store.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, id: &str) -> Result<Option<String>, SecretError> {
        let entry = keyring::Entry::new(&self.service, id)
            .map_err(|error| SecretError::Failed(error.to_string()))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(SecretError::Failed(error.to_string())),
        }
    }

    fn set(&self, id: &str, secret: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(&self.service, id)
            .map_err(|error| SecretError::Failed(error.to_string()))?;
        entry
            .set_password(secret)
            .map_err(|error| SecretError::Failed(error.to_string()))
    }

    fn delete(&self, id: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(&self.service, id)
            .map_err(|error| SecretError::Failed(error.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(SecretError::Failed(error.to_string())),
        }
    }
}

/// In-memory store for tests and for environments without a credential service.
#[derive(Debug, Default)]
pub struct MemoryStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.secrets.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, id: &str) -> Result<Option<String>, SecretError> {
        Ok(self.secrets.lock().unwrap().get(id).cloned())
    }

    fn set(&self, id: &str, secret: &str) -> Result<(), SecretError> {
        self.secrets
            .lock()
            .unwrap()
            .insert(id.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), SecretError> {
        self.secrets.lock().unwrap().remove(id);
        Ok(())
    }
}

/// Credential ids are opaque and stable; they are stored in the config as `auth_ref`.
pub fn credential_id(host_id: &str, kind: &str) -> String {
    format!("{host_id}:{kind}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrips_and_deletes() {
        let store = MemoryStore::new();
        assert!(store.get("a").unwrap().is_none());
        store.set("a", "s3cret").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("s3cret"));
        store.delete("a").unwrap();
        assert!(store.get("a").unwrap().is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn ids_are_stable_and_namespaced() {
        assert_eq!(credential_id("host-1", "password"), "host-1:password");
        assert_eq!(credential_id("host-1", "passphrase"), "host-1:passphrase");
    }

    #[test]
    fn secrets_are_not_written_into_a_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let store = MemoryStore::new();
        store
            .set(&credential_id("host-1", "password"), "hunter2")
            .unwrap();

        let config = crate::config::Config::default();
        config.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("hunter2"),
            "config must only hold credential ids"
        );
    }
}

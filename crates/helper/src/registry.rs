//! Session registry keyed by `(client_id, session_id)`.
//!
//! Namespace isolation (design decision D4) lives here: a lookup only succeeds
//! inside the caller's namespace, and `list` never shows sessions of another
//! client. This is a guard against accidents, not a security boundary — anyone
//! with access to the Unix socket can present any `client_id`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default cap on concurrent sessions per daemon (design risk note).
pub const DEFAULT_MAX_SESSIONS: usize = 32;

/// `(client_id, session_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    /// Client namespace.
    pub client_id: String,
    /// Session identifier inside that namespace.
    pub session_id: String,
}

impl SessionKey {
    /// Build a key from its two parts.
    pub fn new(client_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            session_id: session_id.into(),
        }
    }
}

/// What `list` reports about one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub session_id: String,
    /// Shell pid (also the process-group id).
    pub pid: u32,
    /// Terminal width.
    pub cols: u16,
    /// Terminal height.
    pub rows: u16,
    /// Whether the shell is still running.
    pub alive: bool,
    /// Complete lines currently buffered for replay.
    pub lines: u64,
    /// Initial working directory, when one was requested.
    pub cwd: Option<String>,
    /// Creation time, seconds since the Unix epoch.
    pub created_at: u64,
}

/// Errors surfaced to protocol clients as a failed `reply`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    /// No session with that id in the caller's namespace.
    #[error("session not found")]
    NotFound,
    /// The id exists, but belongs to a different client namespace.
    #[error("session belongs to another client namespace")]
    ForeignNamespace,
    /// The daemon already holds the maximum number of sessions.
    #[error("session limit reached ({0})")]
    Limit(usize),
    /// A session with the same key is already registered.
    #[error("session already exists")]
    Exists,
}

/// Anything a registry can store: tests use a lightweight fake.
pub trait Handle: Send + Sync + 'static {
    /// Owning client namespace.
    fn client_id(&self) -> &str;
    /// Session identifier.
    fn session_id(&self) -> &str;
    /// Whether the underlying process is still running.
    fn is_alive(&self) -> bool;
    /// Snapshot for `list`.
    fn summary(&self) -> SessionSummary;
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Thread-safe map of live sessions.
pub struct Registry<T: Handle> {
    sessions: Mutex<HashMap<SessionKey, Arc<T>>>,
    max_sessions: usize,
}

impl<T: Handle> Registry<T> {
    /// Create a registry holding at most `max_sessions` sessions (`0` = unlimited).
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            max_sessions,
        }
    }

    /// Register a session.
    pub fn insert(&self, handle: Arc<T>) -> Result<(), RegistryError> {
        let key = SessionKey::new(
            handle.client_id().to_string(),
            handle.session_id().to_string(),
        );
        let mut sessions = lock(&self.sessions);
        if sessions.contains_key(&key) {
            return Err(RegistryError::Exists);
        }
        if self.max_sessions > 0 && sessions.len() >= self.max_sessions {
            return Err(RegistryError::Limit(self.max_sessions));
        }
        sessions.insert(key, handle);
        Ok(())
    }

    /// Look up a session inside `client_id`'s namespace.
    pub fn get(&self, client_id: &str, session_id: &str) -> Result<Arc<T>, RegistryError> {
        let sessions = lock(&self.sessions);
        let key = SessionKey::new(client_id, session_id);
        if let Some(handle) = sessions.get(&key) {
            return Ok(Arc::clone(handle));
        }
        // Distinguish "unknown" from "belongs to someone else" so the client can
        // log an honest reason; no session data is ever returned.
        if sessions.keys().any(|other| other.session_id == session_id) {
            return Err(RegistryError::ForeignNamespace);
        }
        Err(RegistryError::NotFound)
    }

    /// Remove a session from the caller's namespace and hand it back.
    pub fn remove(&self, client_id: &str, session_id: &str) -> Result<Arc<T>, RegistryError> {
        let mut sessions = lock(&self.sessions);
        let key = SessionKey::new(client_id, session_id);
        if let Some(handle) = sessions.remove(&key) {
            return Ok(handle);
        }
        if sessions.keys().any(|other| other.session_id == session_id) {
            return Err(RegistryError::ForeignNamespace);
        }
        Err(RegistryError::NotFound)
    }

    /// Sessions visible to `client_id`, oldest insertion order preserved.
    pub fn list(&self, client_id: &str) -> Vec<SessionSummary> {
        let sessions = lock(&self.sessions);
        let mut summaries: Vec<SessionSummary> = sessions
            .iter()
            .filter(|(key, _)| key.client_id == client_id)
            .map(|(_, handle)| handle.summary())
            .collect();
        summaries.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        summaries
    }

    /// Number of registered sessions (all namespaces).
    pub fn len(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// Whether any session is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop sessions whose process has exited; returns how many were removed.
    pub fn prune(&self) -> usize {
        let mut sessions = lock(&self.sessions);
        let before = sessions.len();
        sessions.retain(|_, handle| handle.is_alive());
        before - sessions.len()
    }

    /// Remove and return every registered session (used on daemon shutdown).
    pub fn take_all(&self) -> Vec<Arc<T>> {
        let mut sessions = lock(&self.sessions);
        sessions.drain().map(|(_, handle)| handle).collect()
    }
}

/// Helper for summaries: current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Debug)]
    struct FakeSession {
        client_id: String,
        session_id: String,
        alive: Arc<AtomicBool>,
    }

    impl FakeSession {
        fn new(client_id: &str, session_id: &str) -> Arc<Self> {
            Arc::new(Self {
                client_id: client_id.to_string(),
                session_id: session_id.to_string(),
                alive: Arc::new(AtomicBool::new(true)),
            })
        }
    }

    impl Handle for FakeSession {
        fn client_id(&self) -> &str {
            &self.client_id
        }

        fn session_id(&self) -> &str {
            &self.session_id
        }

        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        fn summary(&self) -> SessionSummary {
            SessionSummary {
                session_id: self.session_id.clone(),
                pid: 1,
                cols: 80,
                rows: 24,
                alive: self.is_alive(),
                lines: 0,
                cwd: None,
                created_at: now_secs(),
            }
        }
    }

    #[test]
    fn list_only_shows_the_callers_namespace() {
        let registry = Registry::new(8);
        registry.insert(FakeSession::new("alice", "s1")).unwrap();
        registry.insert(FakeSession::new("alice", "s2")).unwrap();
        registry.insert(FakeSession::new("bob", "s3")).unwrap();

        let alice = registry.list("alice");
        assert_eq!(
            alice
                .iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["s1", "s2"]
        );
        let bob = registry.list("bob");
        assert_eq!(
            bob.iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["s3"]
        );
        assert!(registry.list("carol").is_empty());
    }

    #[test]
    fn same_session_id_may_exist_in_two_namespaces() {
        let registry = Registry::new(8);
        registry.insert(FakeSession::new("alice", "s1")).unwrap();
        registry.insert(FakeSession::new("bob", "s1")).unwrap();
        assert_eq!(registry.get("alice", "s1").unwrap().client_id(), "alice");
        assert_eq!(registry.get("bob", "s1").unwrap().client_id(), "bob");
    }

    #[test]
    fn cross_namespace_get_is_rejected() {
        let registry = Registry::new(8);
        registry.insert(FakeSession::new("alice", "s1")).unwrap();
        assert_eq!(
            registry.get("bob", "s1").unwrap_err(),
            RegistryError::ForeignNamespace
        );
        assert_eq!(
            registry.remove("bob", "s1").unwrap_err(),
            RegistryError::ForeignNamespace
        );
        // Alice's session is untouched.
        assert!(registry.get("alice", "s1").is_ok());
    }

    #[test]
    fn unknown_sessions_are_not_found() {
        let registry: Registry<FakeSession> = Registry::new(8);
        assert_eq!(
            registry.get("alice", "nope").unwrap_err(),
            RegistryError::NotFound
        );
        assert_eq!(
            registry.remove("alice", "nope").unwrap_err(),
            RegistryError::NotFound
        );
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        let registry = Registry::new(8);
        registry.insert(FakeSession::new("alice", "s1")).unwrap();
        assert_eq!(
            registry
                .insert(FakeSession::new("alice", "s1"))
                .unwrap_err(),
            RegistryError::Exists
        );
    }

    #[test]
    fn session_limit_is_enforced() {
        let registry = Registry::new(2);
        registry.insert(FakeSession::new("alice", "s1")).unwrap();
        registry.insert(FakeSession::new("bob", "s2")).unwrap();
        assert_eq!(
            registry
                .insert(FakeSession::new("carol", "s3"))
                .unwrap_err(),
            RegistryError::Limit(2)
        );
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn zero_limit_means_unlimited() {
        let registry = Registry::new(0);
        for index in 0..100 {
            registry
                .insert(FakeSession::new("alice", &format!("s{index}")))
                .unwrap();
        }
        assert_eq!(registry.len(), 100);
    }

    #[test]
    fn prune_drops_exited_sessions() {
        let registry = Registry::new(8);
        let alive = FakeSession::new("alice", "alive");
        let dead = FakeSession::new("alice", "dead");
        registry.insert(Arc::clone(&alive)).unwrap();
        registry.insert(Arc::clone(&dead)).unwrap();
        dead.alive.store(false, Ordering::SeqCst);

        assert_eq!(registry.prune(), 1);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("alice", "alive").is_ok());
        assert_eq!(
            registry.get("alice", "dead").unwrap_err(),
            RegistryError::NotFound
        );
    }

    #[test]
    fn removing_a_session_frees_its_slot() {
        let registry = Registry::new(1);
        registry.insert(FakeSession::new("alice", "s1")).unwrap();
        assert!(registry.insert(FakeSession::new("alice", "s2")).is_err());
        registry.remove("alice", "s1").unwrap();
        registry.insert(FakeSession::new("alice", "s2")).unwrap();
    }

    #[test]
    fn session_key_is_hashable_by_value() {
        let key = SessionKey::new("a", "b");
        assert_eq!(key, SessionKey::new("a".to_string(), "b".to_string()));
        let mut map = HashMap::new();
        map.insert(key.clone(), 1);
        assert_eq!(map.get(&key), Some(&1));
    }
}

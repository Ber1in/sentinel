//! Session registry: turns wire requests into live [`SessionRuntime`]s and fans their
//! output and state changes out to every connected GUI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use protocol::{
    AuthMethod, ErrorCode, HostSpec, ProtocolError, ProviderKind, SessionState, SessionSummary,
};
use ssh::{ConnectOptions, Credentials, HostKeyPolicy, Keepalive, KnownHosts};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::credentials::SecretStore;
use crate::runner::{Command, OutputChunk, SessionRuntime, SessionSpec};

/// One outbound terminal chunk tagged with the numeric session id used in binary frames.
#[derive(Debug, Clone)]
pub struct RoutedChunk {
    pub session: u32,
    pub seq: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct StateChange {
    pub session_id: String,
    pub state: SessionState,
    pub detail: Option<String>,
}

struct Managed {
    numeric: u32,
    summary: SessionSummary,
    runtime: SessionRuntime,
    watcher: JoinHandle<()>,
    /// Last accepted input, used to drop duplicate deliveries from the client.
    last_input: Mutex<(Vec<u8>, std::time::Instant)>,
}

/// Two identical payloads closer than this are treated as a duplicate delivery.
const DUPLICATE_INPUT_WINDOW: std::time::Duration = std::time::Duration::from_millis(10);

pub struct Manager {
    /// Short client id used in remote session names.
    namespace: String,
    /// Per-host helper install consent, persisted as JSON.
    consents: Mutex<std::collections::HashMap<String, bool>>,
    consent_path: std::path::PathBuf,
    sessions: Mutex<HashMap<String, Managed>>,
    store: Mutex<store::Store>,
    next_numeric: AtomicU32,
    secrets: Arc<dyn SecretStore>,
    config: Config,
    known_hosts: Arc<Mutex<KnownHosts>>,
    chunks: broadcast::Sender<RoutedChunk>,
    changes: broadcast::Sender<StateChange>,
}

impl Manager {
    pub fn new(
        namespace: impl Into<String>,
        secrets: Arc<dyn SecretStore>,
        config: Config,
        known_hosts: KnownHosts,
        store: store::Store,
    ) -> Arc<Self> {
        let consent_path = std::env::var_os("SENTINEL_CONSENT_FILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("helper-consent.json"));
        let (chunks, _) = broadcast::channel(512);
        let (changes, _) = broadcast::channel(256);
        Arc::new(Self {
            namespace: namespace.into(),
            consents: Mutex::new(load_consents(&consent_path)),
            consent_path,
            sessions: Mutex::new(HashMap::new()),
            store: Mutex::new(store),
            next_numeric: AtomicU32::new(1),
            secrets,
            config,
            known_hosts: Arc::new(Mutex::new(known_hosts)),
            chunks,
            changes,
        })
    }

    pub fn subscribe_chunks(&self) -> broadcast::Receiver<RoutedChunk> {
        self.chunks.subscribe()
    }

    pub fn subscribe_changes(&self) -> broadcast::Receiver<StateChange> {
        self.changes.subscribe()
    }

    /// Create or update a saved host. Validation happens here so the GUI cannot store
    /// entries that would fail on every connect attempt.
    pub fn update_host(
        &self,
        host: &HostSpec,
        name: &str,
        group_name: Option<String>,
    ) -> Result<(), ProtocolError> {
        if host.hostname.trim().is_empty() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "hostname must not be empty",
            ));
        }
        if host.username.trim().is_empty() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "username must not be empty",
            ));
        }
        if host.port == 0 {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "port must be between 1 and 65535",
            ));
        }
        if matches!(&host.auth, AuthMethod::Key { key_path, .. } if key_path.trim().is_empty()) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "key path must not be empty",
            ));
        }

        let existing = self
            .store
            .lock()
            .unwrap()
            .host(&host.host_id)
            .ok()
            .flatten();
        let record = store::HostRecord {
            id: host.host_id.clone(),
            name: if name.trim().is_empty() {
                format!("{}@{}", host.username, host.hostname)
            } else {
                name.trim().to_string()
            },
            hostname: host.hostname.trim().to_string(),
            port: host.port,
            username: host.username.trim().to_string(),
            auth_ref: Some(encode_auth(&host.auth)),
            group_name,
            last_used_at: existing
                .map(|record| record.last_used_at)
                .unwrap_or_else(now_millis),
        };
        self.store
            .lock()
            .unwrap()
            .upsert_host(&record)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
    }

    /// Remove a host and detach its live sessions (remote tmux sessions are left alone,
    /// so the user can still attach manually).
    pub fn delete_host(&self, host_id: &str) -> Result<(), ProtocolError> {
        let ids: Vec<String> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, managed)| managed.summary.host_id == host_id)
            .map(|(id, _)| id.clone())
            .collect();

        for id in ids {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(managed) = sessions.remove(&id) {
                managed.watcher.abort();
                managed.runtime.try_send(Command::Detach);
            }
        }

        self.store
            .lock()
            .unwrap()
            .delete_host(host_id)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
    }

    /// Local bundle facts plus the stored consent, shown before installing.
    pub fn helper_info(&self, host_id: &str) -> protocol::HelperInfoView {
        let dir = crate::deploy::helper_dir();
        let target = "x86_64-unknown-linux-musl";
        let path = dir.join(format!("helper-{target}"));
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let sha256 = std::fs::read_to_string(path.with_extension("sha256"))
            .ok()
            .and_then(|text| text.split_whitespace().next().map(str::to_string))
            .unwrap_or_default();
        protocol::HelperInfoView {
            available: path.exists(),
            install_path: "~/.local/share/sentinel/bin/sentinel-helper".into(),
            size,
            sha256,
            consent: self.consents.lock().unwrap().get(host_id).copied(),
        }
    }

    pub fn set_helper_consent(&self, host_id: &str, allow: bool) -> Result<(), ProtocolError> {
        {
            let mut consents = self.consents.lock().unwrap();
            consents.insert(host_id.to_string(), allow);
            let json = serde_json::to_string_pretty(&*consents)
                .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))?;
            std::fs::write(&self.consent_path, json)
                .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))?;
        }
        Ok(())
    }

    fn helper_allowed(&self, host_id: &str) -> bool {
        let explicit = self.consents.lock().unwrap().get(host_id).copied();
        match explicit {
            Some(answer) => answer,
            // No answer yet: keep the previous behaviour (tmux) until the user agrees.
            None => false,
        }
    }

    pub fn store_secret(&self, id: &str, secret: &str) -> Result<(), ProtocolError> {
        self.secrets
            .set(id, secret)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
    }

    /// Copy a host row and, when it uses a stored secret, copy the secret to the new id.
    pub fn duplicate_host(&self, host_id: &str, new_id: &str) -> Result<(), ProtocolError> {
        let source = self
            .store
            .lock()
            .unwrap()
            .host(host_id)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))?
            .ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "host not found"))?;

        let mut auth = decode_auth(source.auth_ref.as_deref().unwrap_or("agent"));
        let mut secret_to_copy = None;
        match &auth {
            AuthMethod::Password { credential_id } => {
                secret_to_copy = self.secrets.get(credential_id).ok().flatten();
            }
            AuthMethod::KeyboardInteractive {
                credential_id: Some(credential_id),
            } => {
                secret_to_copy = self.secrets.get(credential_id).ok().flatten();
            }
            _ => {}
        }

        if let Some(secret) = secret_to_copy {
            let new_credential = crate::credentials::credential_id(new_id, "password");
            self.secrets
                .set(&new_credential, &secret)
                .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))?;
            auth = match auth {
                AuthMethod::KeyboardInteractive { .. } => AuthMethod::KeyboardInteractive {
                    credential_id: Some(new_credential),
                },
                _ => AuthMethod::Password {
                    credential_id: new_credential,
                },
            };
        }

        let record = store::HostRecord {
            id: new_id.to_string(),
            name: format!("{} (copy)", source.name),
            hostname: source.hostname,
            port: source.port,
            username: source.username,
            auth_ref: Some(encode_auth(&auth)),
            group_name: source.group_name,
            last_used_at: now_millis(),
        };
        self.store
            .lock()
            .unwrap()
            .upsert_host(&record)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
    }

    pub fn delete_secret(&self, id: &str) -> Result<(), ProtocolError> {
        self.secrets
            .delete(id)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
    }

    pub fn save_layout(
        &self,
        tabs: &[protocol::TabLayout],
        panes: &[protocol::PaneLayout],
    ) -> Result<(), ProtocolError> {
        let tabs: Vec<store::TabRecord> = tabs
            .iter()
            .map(|tab| store::TabRecord {
                id: tab.id.clone(),
                title: tab.title.clone(),
                sort_order: tab.sort_order,
            })
            .collect();
        let panes: Vec<store::PaneRecord> = panes
            .iter()
            .map(|pane| store::PaneRecord {
                id: pane.id.clone(),
                tab_id: pane.tab_id.clone(),
                parent_id: pane.parent_id.clone(),
                direction: pane.direction.as_deref().map(|value| {
                    if value == "vertical" {
                        store::SplitDirection::Vertical
                    } else {
                        store::SplitDirection::Horizontal
                    }
                }),
                ratio: pane.ratio,
                host_id: pane.host_id.clone(),
                session_id: pane.session_id.clone(),
                sort_order: pane.sort_order,
            })
            .collect();
        self.store
            .lock()
            .unwrap()
            .save_layout(&tabs, &panes)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
    }

    pub fn load_layout(
        &self,
    ) -> Result<(Vec<protocol::TabLayout>, Vec<protocol::PaneLayout>), ProtocolError> {
        let (tabs, panes) = self
            .store
            .lock()
            .unwrap()
            .load_layout()
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))?;
        Ok((
            tabs.into_iter()
                .map(|tab| protocol::TabLayout {
                    id: tab.id,
                    title: tab.title,
                    sort_order: tab.sort_order,
                })
                .collect(),
            panes
                .into_iter()
                .map(|pane| protocol::PaneLayout {
                    id: pane.id,
                    tab_id: pane.tab_id,
                    parent_id: pane.parent_id,
                    direction: pane.direction.map(|direction| match direction {
                        store::SplitDirection::Horizontal => "horizontal".to_string(),
                        store::SplitDirection::Vertical => "vertical".to_string(),
                    }),
                    ratio: pane.ratio,
                    host_id: pane.host_id,
                    session_id: pane.session_id,
                    sort_order: pane.sort_order,
                })
                .collect(),
        ))
    }

    pub fn hosts(&self) -> Vec<protocol::HostSummary> {
        self.store
            .lock()
            .unwrap()
            .hosts()
            .unwrap_or_default()
            .into_iter()
            .map(|host| protocol::HostSummary {
                id: host.id,
                name: host.name,
                hostname: host.hostname,
                port: host.port,
                username: host.username,
                auth: decode_auth(host.auth_ref.as_deref().unwrap_or("agent")),
                group_name: host.group_name,
                last_used_at: host.last_used_at,
            })
            .collect()
    }

    fn remember_host(&self, host: &HostSpec) {
        let record = store::HostRecord {
            id: host.host_id.clone(),
            name: format!("{}@{}", host.username, host.hostname),
            hostname: host.hostname.clone(),
            port: host.port,
            username: host.username.clone(),
            auth_ref: Some(match &host.auth {
                AuthMethod::Password { credential_id }
                | AuthMethod::KeyboardInteractive {
                    credential_id: Some(credential_id),
                } => credential_id.clone(),
                AuthMethod::Key { key_path, .. } => key_path.clone(),
                _ => String::new(),
            }),
            group_name: None,
            last_used_at: now_millis(),
        };
        let _ = self.store.lock().unwrap().upsert_host(&record);
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .map(|managed| SessionSummary {
                numeric: managed.numeric,
                ..managed.summary.clone()
            })
            .collect()
    }

    pub fn session_by_numeric(&self, numeric: u32) -> Option<String> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .find(|(_, managed)| managed.numeric == numeric)
            .map(|(id, _)| id.clone())
    }

    pub fn numeric_id(&self, session_id: &str) -> Option<u32> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|managed| managed.numeric)
    }

    /// Recent output for a GUI that just attached.
    pub fn replay(&self, session_id: &str, lines: usize) -> Option<(u32, Vec<u8>)> {
        let sessions = self.sessions.lock().unwrap();
        let managed = sessions.get(session_id)?;
        Some((managed.numeric, managed.runtime.replay(lines)))
    }

    pub fn create(
        &self,
        host: &HostSpec,
        pane_id: String,
        cwd: Option<String>,
    ) -> Result<SessionSummary, ProtocolError> {
        let credentials = self.resolve_credentials(host)?;
        let session_id = format!("{}:{}", host.host_id, pane_id);

        // Reopening the GUI (or clicking the same host again) must attach to the live
        // session instead of failing or creating a second shell.
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(managed) = sessions.get(&session_id) {
                if *managed.runtime.events.state.borrow() == SessionState::Connected {
                    return Ok(managed.summary.clone());
                }
                // A dead runtime cannot be reused; drop it and reconnect from scratch.
                let dead = sessions.remove(&session_id).unwrap();
                dead.watcher.abort();
                dead.runtime.try_send(Command::Shutdown);
            }
        }

        let provider_kind =
            if self.helper_allowed(&host.host_id) && self.helper_info(&host.host_id).available {
                ProviderKind::Helper
            } else {
                ProviderKind::Tmux
            };

        let provider: Arc<dyn persistence::PersistenceProvider> = Arc::new(
            persistence::TmuxPersistence::new(persistence::TmuxOptions {
                status: self.config.tmux.status,
                mouse: self.config.tmux.mouse,
                history_limit: self.config.tmux.history_limit,
                default_terminal: self.config.tmux.default_terminal.clone(),
                ..Default::default()
            }),
        );

        let spec = SessionSpec {
            session_id: session_id.clone(),
            namespace: self.namespace.clone(),
            host_id: host.host_id.clone(),
            pane_id,
            connect: ConnectOptions {
                host: host.hostname.clone(),
                port: host.port,
                user: host.username.clone(),
                credentials,
                host_key_policy: HostKeyPolicy::TrustOnFirstUse(self.known_hosts.clone()),
                keepalive: Keepalive {
                    interval: std::time::Duration::from_secs(self.config.keepalive_seconds),
                    max_misses: 3,
                },
                timeout: std::time::Duration::from_secs(15),
            },
            provider,
            provider_kind,
            client_id: self.namespace.clone(),
            cwd: cwd.clone(),
            scrollback_lines: self.config.scrollback_lines,
            spill_dir: None,
            spill_max_bytes: 50 * 1024 * 1024,
            policy: crate::fsm::ReconnectPolicy::default(),
            term: "xterm-256color".into(),
            cols: 120,
            rows: 32,
        };

        self.remember_host(host);
        let numeric = self.next_numeric.fetch_add(1, Ordering::Relaxed);
        let runtime = SessionRuntime::spawn(spec);
        let summary = SessionSummary {
            session_id: session_id.clone(),
            numeric,
            host_id: host.host_id.clone(),
            title: format!("{}@{}", host.username, host.hostname),
            state: SessionState::Connecting,
            provider: ProviderKind::Tmux,
            cwd,
            last_active_at: now_millis(),
        };

        let watcher = spawn_forwarders(
            session_id.clone(),
            numeric,
            runtime.events.state.clone(),
            runtime.events.state_detail.clone(),
            runtime.events.output.subscribe(),
            self.chunks.clone(),
            self.changes.clone(),
        );

        self.sessions.lock().unwrap().insert(
            session_id,
            Managed {
                numeric,
                summary: summary.clone(),
                runtime,
                watcher,
                last_input: Mutex::new((Vec::new(), std::time::Instant::now())),
            },
        );
        Ok(summary)
    }

    /// Detach every session without killing remote processes. Used before a graceful
    /// restart so tmux sessions survive the daemon swap.
    pub fn shutdown_all(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        for (_, managed) in sessions.drain() {
            managed.watcher.abort();
            managed.runtime.try_send(Command::Detach);
        }
    }

    pub fn send_input(&self, session_id: &str, bytes: Vec<u8>) -> bool {
        let sessions = self.sessions.lock().unwrap();
        let Some(managed) = sessions.get(session_id) else {
            return false;
        };
        managed.runtime.try_send(Command::Input(bytes))
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> bool {
        let sessions = self.sessions.lock().unwrap();
        let Some(managed) = sessions.get(session_id) else {
            return false;
        };
        managed.runtime.try_send(Command::Resize { cols, rows })
    }

    pub fn detach(&self, session_id: &str) -> bool {
        let sessions = self.sessions.lock().unwrap();
        let Some(managed) = sessions.get(session_id) else {
            return false;
        };
        managed.runtime.try_send(Command::Detach)
    }

    pub fn destroy(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(managed) = sessions.remove(session_id) else {
            return false;
        };
        managed.watcher.abort();
        managed.runtime.try_send(Command::Destroy)
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_forwarders(
    session_id: String,
    numeric: u32,
    mut states: watch::Receiver<SessionState>,
    details: watch::Receiver<Option<String>>,
    mut output: broadcast::Receiver<OutputChunk>,
    chunks: broadcast::Sender<RoutedChunk>,
    changes: broadcast::Sender<StateChange>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Report the state the session started in.
        let _ = changes.send(StateChange {
            session_id: session_id.clone(),
            state: *states.borrow(),
            detail: details.borrow().clone(),
        });
        loop {
            tokio::select! {
                changed = states.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let _ = changes.send(StateChange {
                        session_id: session_id.clone(),
                        state: *states.borrow(),
                        detail: details.borrow().clone(),
                    });
                }
                chunk = output.recv() => {
                    match chunk {
                        Ok(chunk) => {
                            let _ = chunks.send(RoutedChunk {
                                session: numeric,
                                seq: chunk.seq,
                                bytes: chunk.bytes,
                            });
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        }
    })
}

impl Manager {
    fn resolve_credentials(&self, host: &HostSpec) -> Result<Credentials, ProtocolError> {
        let secret = |id: &str| {
            self.secrets
                .get(id)
                .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
        };
        match &host.auth {
            AuthMethod::Password { credential_id } => {
                let password = secret(credential_id)?.ok_or_else(|| {
                    ProtocolError::new(ErrorCode::AuthFailed, "password not stored")
                })?;
                Ok(Credentials::Password(password))
            }
            AuthMethod::Key {
                key_path,
                credential_id,
            } => {
                let passphrase = match credential_id {
                    Some(id) => secret(id)?,
                    None => None,
                };
                Ok(Credentials::Key {
                    path: Some(key_path.clone().into()),
                    pem: None,
                    passphrase,
                })
            }
            AuthMethod::Agent => Ok(Credentials::Agent),
            AuthMethod::KeyboardInteractive { credential_id } => {
                let secret = match credential_id {
                    Some(id) => secret(id)?,
                    None => None,
                };
                let responder: ssh::KiResponder = Arc::new(move |request: &ssh::KiRequest| {
                    let value = secret.clone().unwrap_or_default();
                    vec![value; request.prompts.len()]
                });
                Ok(Credentials::KeyboardInteractive { responder })
            }
        }
    }
}

/// Persisted auth descriptors keep the method, so the edit dialog can prefill it.
fn encode_auth(auth: &AuthMethod) -> String {
    match auth {
        AuthMethod::Password { credential_id } => format!("password:{credential_id}"),
        AuthMethod::Key {
            key_path,
            credential_id,
        } => {
            format!(
                "key:{key_path}|{}",
                credential_id.clone().unwrap_or_default()
            )
        }
        AuthMethod::Agent => "agent".to_string(),
        AuthMethod::KeyboardInteractive { credential_id } => {
            format!("kbd:{}", credential_id.clone().unwrap_or_default())
        }
    }
}

fn decode_auth(raw: &str) -> AuthMethod {
    if let Some(rest) = raw.strip_prefix("password:") {
        return AuthMethod::Password {
            credential_id: rest.to_string(),
        };
    }
    if let Some(rest) = raw.strip_prefix("key:") {
        let (path, credential) = rest.split_once('|').unwrap_or((rest, ""));
        return AuthMethod::Key {
            key_path: path.to_string(),
            credential_id: (!credential.is_empty()).then(|| credential.to_string()),
        };
    }
    // Rows written before auth descriptors were encoded stored a bare credential id.
    if raw.ends_with(":password") && raw.len() > ":password".len() {
        return AuthMethod::Password {
            credential_id: raw.to_string(),
        };
    }
    if let Some(rest) = raw.strip_prefix("kbd:") {
        return AuthMethod::KeyboardInteractive {
            credential_id: (!rest.is_empty()).then(|| rest.to_string()),
        };
    }
    AuthMethod::Agent
}

fn load_consents(path: &std::path::Path) -> std::collections::HashMap<String, bool> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
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
    use crate::credentials::MemoryStore;

    fn manager() -> Arc<Manager> {
        Manager::new(
            "testns",
            Arc::new(MemoryStore::new()),
            Config::default(),
            KnownHosts::in_memory(),
            store::Store::in_memory().unwrap(),
        )
    }

    fn host(id: &str) -> HostSpec {
        HostSpec {
            host_id: id.into(),
            hostname: "10.0.0.9".into(),
            port: 2222,
            username: "root".into(),
            auth: AuthMethod::Password {
                credential_id: format!("{id}:password"),
            },
        }
    }

    #[test]
    fn updating_a_host_persists_and_reports_auth_method() {
        let manager = manager();
        manager
            .update_host(&host("h1"), "prod", Some("servers".into()))
            .unwrap();
        let hosts = manager.hosts();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "prod");
        assert_eq!(hosts[0].port, 2222);
        assert_eq!(hosts[0].group_name.as_deref(), Some("servers"));
        assert!(matches!(hosts[0].auth, AuthMethod::Password { .. }));
    }

    #[test]
    fn invalid_hosts_are_rejected_with_a_readable_code() {
        let manager = manager();
        let mut broken = host("h1");
        broken.hostname = "  ".into();
        let error = manager.update_host(&broken, "x", None).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);

        let mut broken = host("h1");
        broken.port = 0;
        assert_eq!(
            manager.update_host(&broken, "x", None).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn empty_display_name_falls_back_to_user_at_host() {
        let manager = manager();
        manager.update_host(&host("h1"), "   ", None).unwrap();
        assert_eq!(manager.hosts()[0].name, "root@10.0.0.9");
    }

    #[test]
    fn duplicate_copies_the_host_and_its_secret() {
        let manager = manager();
        manager.update_host(&host("h1"), "prod", None).unwrap();
        manager.store_secret("h1:password", "hunter2").unwrap();

        manager.duplicate_host("h1", "h2").unwrap();
        let hosts = manager.hosts();
        assert_eq!(hosts.len(), 2);
        let copy = hosts.iter().find(|entry| entry.id == "h2").unwrap();
        assert!(copy.name.ends_with("(copy)"));
        assert_eq!(
            manager.secrets.get("h2:password").unwrap().as_deref(),
            Some("hunter2")
        );
    }

    #[test]
    fn layout_roundtrips_through_the_manager() {
        let manager = manager();
        let tabs = vec![protocol::TabLayout {
            id: "t1".into(),
            title: "prod".into(),
            sort_order: 0,
        }];
        let panes = vec![protocol::PaneLayout {
            id: "p1".into(),
            tab_id: "t1".into(),
            parent_id: None,
            direction: Some("vertical".into()),
            ratio: 0.62,
            host_id: Some("h1".into()),
            session_id: Some("h1:main".into()),
            sort_order: 0,
        }];
        manager.save_layout(&tabs, &panes).unwrap();
        let (loaded_tabs, loaded_panes) = manager.load_layout().unwrap();
        assert_eq!(loaded_tabs, tabs);
        assert_eq!(loaded_panes, panes);
    }

    #[test]
    fn saving_a_layout_replaces_the_previous_one() {
        let manager = manager();
        manager
            .save_layout(
                &[protocol::TabLayout {
                    id: "a".into(),
                    title: "a".into(),
                    sort_order: 0,
                }],
                &[],
            )
            .unwrap();
        manager
            .save_layout(
                &[protocol::TabLayout {
                    id: "b".into(),
                    title: "b".into(),
                    sort_order: 0,
                }],
                &[],
            )
            .unwrap();
        let (tabs, _) = manager.load_layout().unwrap();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, "b");
    }

    #[test]
    fn deleting_a_host_removes_it() {
        let manager = manager();
        manager.update_host(&host("h1"), "prod", None).unwrap();
        manager.delete_host("h1").unwrap();
        assert!(manager.hosts().is_empty());
    }

    #[test]
    fn deleting_a_missing_host_is_not_an_error() {
        let manager = manager();
        manager.delete_host("nope").unwrap();
    }
}

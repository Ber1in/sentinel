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
}

pub struct Manager {
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
        secrets: Arc<dyn SecretStore>,
        config: Config,
        known_hosts: KnownHosts,
        store: store::Store,
    ) -> Arc<Self> {
        let (chunks, _) = broadcast::channel(512);
        let (changes, _) = broadcast::channel(256);
        Arc::new(Self {
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

    pub fn store_secret(&self, id: &str, secret: &str) -> Result<(), ProtocolError> {
        self.secrets
            .set(id, secret)
            .map_err(|error| ProtocolError::new(ErrorCode::Internal, error.to_string()))
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
            .map(|managed| managed.summary.clone())
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
        if self.sessions.lock().unwrap().contains_key(&session_id) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "session already exists for this pane",
            ));
        }

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
            },
        );
        Ok(summary)
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

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

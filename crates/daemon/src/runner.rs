//! One live pane: SSH connection, remote persistence binding, output fan-out, and the
//! reconnect loop.
//!
//! The runner owns everything that makes a session survive: it reconnects with backoff,
//! replays remote history after reattaching, and reports when the remote session had to
//! be recreated (which means the user's processes are gone).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use persistence::{
    CommandOutput, EnsureOutcome, PersistenceError, PersistenceProvider, RemoteExec,
};
use protocol::SessionState;
use russh::ChannelMsg;
use ssh::{ConnectOptions, Connection};
use terminal::{Scrollback, SpillStore};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::fsm::{Event, ReconnectPolicy, SessionFsm};
use crate::input::InputQueue;

/// Replay window sent to a GUI that attaches or reconnects.
pub const REPLAY_LINES: usize = 2_000;

#[derive(Debug, Clone)]
pub struct OutputChunk {
    pub seq: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum Command {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
    Destroy,
    Shutdown,
}

#[derive(Clone)]
pub struct SessionSpec {
    pub session_id: String,
    /// Client-installation namespace; keeps remote session names globally unique.
    pub namespace: String,
    pub host_id: String,
    pub pane_id: String,
    pub connect: ConnectOptions,
    pub provider: Arc<dyn PersistenceProvider>,
    /// Transport that carries the pane.
    pub provider_kind: protocol::ProviderKind,
    /// Client identity used by the helper namespace.
    pub client_id: String,
    pub cwd: Option<String>,
    pub scrollback_lines: usize,
    pub spill_dir: Option<PathBuf>,
    pub spill_max_bytes: u64,
    pub policy: ReconnectPolicy,
    pub term: String,
    pub cols: u16,
    pub rows: u16,
}

pub struct SessionEvents {
    pub state: watch::Receiver<SessionState>,
    /// Set to true when the last successful attach had to recreate the remote session.
    pub recreated: watch::Receiver<bool>,
    pub output: broadcast::Sender<OutputChunk>,
    pub state_detail: watch::Receiver<Option<String>>,
    /// Working directory reported by shell integration, when the remote shell emits it.
    pub cwd: watch::Receiver<Option<String>>,
}

pub struct SessionRuntime {
    pub session_id: String,
    pub events: SessionEvents,
    pub scrollback: Arc<Mutex<Scrollback>>,
    commands: mpsc::Sender<Command>,
    recreated_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl SessionRuntime {
    pub fn spawn(spec: SessionSpec) -> Self {
        let scrollback = Arc::new(Mutex::new(Scrollback::new(spec.scrollback_lines)));
        let (state_tx, state_rx) = watch::channel(SessionState::Connecting);
        let (recreated_tx, recreated_rx) = watch::channel(false);
        let (detail_tx, detail_rx) = watch::channel(None);
        let (cwd_tx, cwd_rx) = watch::channel(None);
        let (output_tx, _) = broadcast::channel(256);
        let (command_tx, command_rx) = mpsc::channel(64);

        let task = tokio::spawn(run_loop(
            spec.clone(),
            scrollback.clone(),
            state_tx,
            recreated_tx.clone(),
            detail_tx,
            cwd_tx,
            output_tx.clone(),
            command_rx,
        ));

        Self {
            session_id: spec.session_id,
            events: SessionEvents {
                state: state_rx,
                recreated: recreated_rx,
                output: output_tx,
                state_detail: detail_rx,
                cwd: cwd_rx,
            },
            scrollback,
            commands: command_tx,
            recreated_tx,
            task,
        }
    }

    pub async fn send(&self, command: Command) -> bool {
        self.commands.send(command).await.is_ok()
    }

    /// Non-blocking variant for callers that are not async (IPC read loop).
    pub fn try_send(&self, command: Command) -> bool {
        self.commands.try_send(command).is_ok()
    }

    /// Recent output for a fresh or reconnecting GUI.
    pub fn replay(&self, lines: usize) -> Vec<u8> {
        self.scrollback.lock().unwrap().tail_lines(lines)
    }

    /// True once the runtime loop has stopped (detached, failed or shut down).
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Acknowledge the `recreated` signal once the GUI has shown it.
    pub fn clear_recreated(&self) {
        let _ = self.recreated_tx.send(false);
    }

    pub fn abort(self) {
        self.task.abort();
    }
}

struct RemoteExecOverSsh {
    connection: Arc<Connection>,
}

#[async_trait]
impl RemoteExec for RemoteExecOverSsh {
    async fn run(&self, command: &str) -> Result<CommandOutput, PersistenceError> {
        let mut channel = self
            .connection
            .exec(command)
            .await
            .map_err(|error| PersistenceError::Transport(error.to_string()))?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code = 0;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status as i32,
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(CommandOutput {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

enum RunOutcome {
    /// The user detached or asked to shut down.
    Finished,
    Retry(String),
    Fatal(String),
}

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    spec: SessionSpec,
    scrollback: Arc<Mutex<Scrollback>>,
    state_tx: watch::Sender<SessionState>,
    recreated_tx: watch::Sender<bool>,
    detail_tx: watch::Sender<Option<String>>,
    cwd_tx: watch::Sender<Option<String>>,
    output: broadcast::Sender<OutputChunk>,
    mut commands: mpsc::Receiver<Command>,
) {
    let mut fsm = SessionFsm::new(spec.policy);
    let mut spill = spec
        .spill_dir
        .as_ref()
        .and_then(|dir| SpillStore::open(dir, &spec.session_id, spec.spill_max_bytes).ok());
    let mut queued = InputQueue::default();
    let session_name = spec
        .provider
        .session_name(&spec.namespace, &spec.host_id, &spec.pane_id);

    let publish = |state: SessionState| {
        let _ = state_tx.send(state);
    };
    publish(fsm.state());

    loop {
        match run_once(
            &spec,
            &session_name,
            &scrollback,
            &output,
            &cwd_tx,
            &state_tx,
            &recreated_tx,
            &mut spill,
            &mut commands,
            &mut queued,
            &mut fsm,
        )
        .await
        {
            RunOutcome::Finished => {
                if let Some(store) = spill.take() {
                    if matches!(fsm.state(), SessionState::Detached) {
                        let _ = store.destroy();
                    }
                }
                publish(fsm.state());
                return;
            }
            RunOutcome::Fatal(detail) => {
                let _ = fsm.apply(Event::Fatal);
                let _ = detail_tx.send(Some(detail));
                publish(fsm.state());
                return;
            }
            RunOutcome::Retry(detail) => {
                if fsm.state() == SessionState::Connected {
                    let _ = fsm.apply(Event::LinkLost);
                }
                let _ = detail_tx.send(Some(detail));
                publish(fsm.state());
                let delay = fsm.reconnect_delay();
                tokio::time::sleep(delay).await;
                let _ = fsm.apply(Event::RetryDue);
                publish(fsm.state());
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_once(
    spec: &SessionSpec,
    session_name: &str,
    scrollback: &Arc<Mutex<Scrollback>>,
    output: &broadcast::Sender<OutputChunk>,
    cwd_tx: &watch::Sender<Option<String>>,
    state_tx: &watch::Sender<SessionState>,
    recreated_tx: &watch::Sender<bool>,
    spill: &mut Option<SpillStore>,
    commands: &mut mpsc::Receiver<Command>,
    queued: &mut InputQueue,
    fsm: &mut SessionFsm,
) -> RunOutcome {
    tracing::debug!(session = %spec.session_id, "connecting");
    let connection = match Connection::connect(&spec.connect).await {
        Ok(connection) => Arc::new(connection),
        Err(error) => {
            tracing::warn!(session = %spec.session_id, %error, "connect failed");
            return if error.retryable() {
                RunOutcome::Retry(error.to_string())
            } else {
                RunOutcome::Fatal(error.to_string())
            };
        }
    };

    let mut cwd_tracker = terminal::CwdTracker::new();
    let mut channel: Box<dyn crate::pane_channel::PaneChannel> =
        if spec.provider_kind == protocol::ProviderKind::Helper {
            // Deploy on demand: the first connection installs the helper, later ones
            // reuse it. The binaries ship next to the daemon in the installer.
            let helper_dir = crate::deploy::helper_dir();
            if let Err(error) = crate::deploy::ensure(&connection, &helper_dir).await {
                tracing::warn!(
                    session = %spec.session_id,
                    dir = %helper_dir.display(),
                    %error,
                    "helper deployment failed"
                );
                // Retrying a missing bundle forever would leave the tab stuck on
                // "connecting"; report the reason instead.
                return RunOutcome::Fatal(format!("helper deployment failed: {error}"));
            }

            match crate::frame_pane::FramePane::open(
                &connection,
                &spec.client_id,
                &spec.session_id,
                spec.cwd.as_deref(),
                spec.cols,
                spec.rows,
            )
            .await
            {
                Ok(pane) => Box::new(pane),
                Err(error) => {
                    tracing::warn!(session = %spec.session_id, %error, "helper pane failed");
                    return RunOutcome::Retry(error.to_string());
                }
            }
        } else {
            let exec = RemoteExecOverSsh {
                connection: connection.clone(),
            };
            let outcome = match spec
                .provider
                .ensure(&exec, session_name, spec.cwd.as_deref())
                .await
            {
                Ok(outcome) => {
                    tracing::debug!(session = %spec.session_id, ?outcome, "persistence bound");
                    outcome
                }
                Err(PersistenceError::TmuxMissing) => {
                    return RunOutcome::Fatal("tmux is not installed on the host".into())
                }
                Err(error) => {
                    tracing::warn!(session = %spec.session_id, %error, "ensure failed");
                    return RunOutcome::Retry(error.to_string());
                }
            };

            // Replay remote history before attaching so a reconnecting GUI sees what it missed.
            if outcome == EnsureOutcome::Attached {
                if let Ok(snapshot) = spec
                    .provider
                    .snapshot(&exec, session_name, REPLAY_LINES)
                    .await
                {
                    if !snapshot.is_empty() {
                        // The remote screen is authoritative after a reattach: clear the client
                        // view first so the snapshot does not stack on top of stale content.
                        let mut replay = b"\x1b[2J\x1b[H".to_vec();
                        replay.extend_from_slice(snapshot.as_bytes());
                        let seq = scrollback.lock().unwrap().append(&replay);
                        let _ = output.send(OutputChunk { seq, bytes: replay });
                    }
                }
            } else if outcome == EnsureOutcome::Recreated {
                fsm.mark_recreated();
                let _ = recreated_tx.send(true);
            } else {
                let _ = recreated_tx.send(false);
            }

            let exec_channel = match connection
                .open_pty_exec(
                    &spec
                        .provider
                        .attach_command(session_name, spec.cwd.as_deref()),
                    &spec.term,
                    spec.cols,
                    spec.rows,
                )
                .await
            {
                Ok(channel) => channel,
                Err(error) => {
                    tracing::warn!(session = %spec.session_id, %error, "attach failed");
                    return RunOutcome::Retry(error.to_string());
                }
            };

            Box::new(crate::pane_channel::ExecPane::new(exec_channel))
        };
    let _ = fsm.apply(Event::Connected);
    let _ = state_tx.send(fsm.state());

    // Drain anything typed while disconnected.
    if !queued.is_empty() {
        let pending = queued.take();
        let _ = channel.write(&pending).await;
    }

    loop {
        tokio::select! {
            message = channel.next() => {
                match message {
                    Some(ChannelMsg::Data { data }) => {
                        if let Some(cwd) = cwd_tracker.feed(&data) {
                            let _ = cwd_tx.send(Some(cwd));
                        }
                        let seq = scrollback.lock().unwrap().append(&data);
                        if let Some(store) = spill.as_mut() {
                            if store.append(seq, &data).is_err() {
                                store.disable();
                            }
                        }
                        let _ = output.send(OutputChunk { seq, bytes: data.to_vec() });
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let seq = scrollback.lock().unwrap().append(&data);
                        let _ = output.send(OutputChunk { seq, bytes: data.to_vec() });
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        return RunOutcome::Retry("remote channel closed".into());
                    }
                    _ => {}
                }
            }
            command = commands.recv() => {
                match command {
                    Some(Command::Input(bytes)) => {
                        if channel.write(&bytes).await.is_err() {
                            let _ = queued.push(&bytes);
                            return RunOutcome::Retry("write failed".into());
                        }
                    }
                    Some(Command::Resize { cols, rows }) => {
                        let _ = channel.resize(cols, rows).await;
                    }
                    Some(Command::Detach) => {
                        let _ = fsm.apply(Event::Detach);
                        let _ = channel.close().await;
                        return RunOutcome::Finished;
                    }
                    Some(Command::Destroy) => {
                        let exec = RemoteExecOverSsh { connection: connection.clone() };
                        let _ = spec.provider.destroy(&exec, session_name).await;
                        let _ = channel.close().await;
                        return RunOutcome::Finished;
                    }
                    Some(Command::Shutdown) | None => {
                        let _ = fsm.apply(Event::Detach);
                        let _ = channel.close().await;
                        return RunOutcome::Finished;
                    }
                }
            }
        }
    }
}

/// Default runtime for a spec with sane dimensions.
pub fn default_spec(spec: SessionSpec) -> SessionRuntime {
    SessionRuntime::spawn(spec)
}

/// Convenience for tests and the daemon: wait until the session reaches `target`.
pub async fn wait_for_state(events: &mut SessionEvents, target: SessionState) -> bool {
    if *events.state.borrow() == target {
        return true;
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if events.state.changed().await.is_err() {
                return false;
            }
            if *events.state.borrow() == target {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false)
}

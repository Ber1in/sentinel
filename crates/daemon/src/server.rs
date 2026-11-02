//! Loopback WebSocket server: handshake gate, request dispatch, and streaming.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, WebSocketStream};

use protocol::{
    decode_data_frame, encode_data_frame, ClientMessage, ClientOp, ErrorCode, FrameKind, OpResult,
    ProtocolError, ServerEvent, ServerInfo, ServerMessage, PROTOCOL_VERSION,
};

use crate::manager::Manager;
use crate::paths::DaemonPaths;
use crate::token::{now_millis, write_daemon_file, DaemonFile};

/// How long a client has to complete the handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Default number of lines replayed to a GUI that attaches.
const REPLAY_LINES: usize = 2_000;

/// Exit code telling the supervisor "restart me, this was intentional".
pub const RESTART_EXIT_CODE: i32 = 75;

pub struct DaemonServer {
    pub addr: SocketAddr,
    pub token: String,
    pub info: ServerInfo,
    task: JoinHandle<()>,
}

impl DaemonServer {
    pub async fn start(paths: &DaemonPaths, manager: Arc<Manager>) -> Result<Self> {
        paths.ensure()?;
        let _ = std::fs::remove_file(paths.daemon_file());

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("bind loopback listener")?;
        let addr = listener.local_addr()?;
        let token = crate::token::generate_token();
        let info = ServerInfo {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
        };

        write_daemon_file(
            &paths.daemon_file(),
            &DaemonFile {
                pid: info.pid,
                port: addr.port(),
                token: token.clone(),
                protocol_version: PROTOCOL_VERSION,
                daemon_version: info.daemon_version.clone(),
                started_at: now_millis(),
            },
        )?;

        let accept_token = token.clone();
        let accept_info = info.clone();
        let task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _peer)) => {
                        let token = accept_token.clone();
                        let info = accept_info.clone();
                        let manager = manager.clone();
                        tokio::spawn(async move {
                            if let Err(error) = serve_connection(stream, token, info, manager).await
                            {
                                tracing::debug!(%error, "client connection ended");
                            }
                        });
                    }
                    Err(error) => tracing::warn!(%error, "accept failed"),
                }
            }
        });

        Ok(Self {
            addr,
            token,
            info,
            task,
        })
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    pub fn abort(&self) {
        self.task.abort();
    }
}

async fn serve_connection(
    stream: TcpStream,
    expected_token: String,
    info: ServerInfo,
    manager: Arc<Manager>,
) -> Result<()> {
    let ws = accept_async(stream).await.context("websocket upgrade")?;
    let ws =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(ws, &expected_token, info)).await {
            Ok(Ok(ws)) => ws,
            Ok(Err(error)) => {
                tracing::info!(%error, "handshake rejected");
                return Ok(());
            }
            Err(_) => {
                tracing::info!("handshake timed out");
                return Ok(());
            }
        };

    let (mut sink, mut incoming) = ws.split();
    let mut changes = manager.subscribe_changes();
    let mut chunks = manager.subscribe_chunks();

    loop {
        tokio::select! {
            message = incoming.next() => {
                let Some(message) = message else { break };
                match message? {
                    Message::Close(_) => break,
                    Message::Ping(payload) => sink.send(Message::Pong(payload)).await?,
                    Message::Text(text) => {
                        // `resync` answers with a binary replay on this connection, so it
                        // cannot go through the generic request dispatcher.
                        let resync = serde_json::from_str::<ClientMessage>(&text).ok().and_then(|message| match message {
                            ClientMessage::Request { id, op: ClientOp::Resync { session_id, .. } } => {
                                Some((id, session_id))
                            }
                            _ => None,
                        });
                        if let Some((id, session_id)) = resync {
                            if let Some((numeric, bytes)) = manager.replay(&session_id, REPLAY_LINES) {
                                if !bytes.is_empty() {
                                    sink.send(Message::Binary(encode_data_frame(
                                        FrameKind::Output,
                                        numeric,
                                        0,
                                        &bytes,
                                    )))
                                    .await?;
                                }
                            }
                            let reply = ServerMessage::Reply { id, result: OpResult::Ok };
                            sink.send(Message::Text(serde_json::to_string(&reply)?)).await?;
                        } else if let Some(reply) = handle_request(&manager, &text) {
                            sink.send(Message::Text(serde_json::to_string(&reply)?)).await?;
                        }
                    }
                    Message::Binary(frame) => {
                        if let Some((FrameKind::Input, numeric, _seq, payload)) =
                            decode_data_frame(&frame)
                        {
                            if let Some(session) = manager.session_by_numeric(numeric) {
                                manager.send_input(&session, payload.to_vec());
                            }
                        }
                    }
                    _ => {}
                }
            }
            change = changes.recv() => {
                if let Ok(change) = change {
                    let event = ServerEvent::SessionState {
                        session_id: change.session_id,
                        state: change.state,
                        detail: change.detail,
                    };
                    sink.send(Message::Text(serde_json::to_string(&ServerMessage::Event { event })?))
                        .await?;
                }
            }
            chunk = chunks.recv() => {
                if let Ok(chunk) = chunk {
                    let frame = encode_data_frame(FrameKind::Output, chunk.session, chunk.seq, &chunk.bytes);
                    sink.send(Message::Binary(frame)).await?;
                }
            }
        }
    }
    Ok(())
}

/// Returns the reply to send, or `None` for fire-and-forget requests.
fn handle_request(manager: &Arc<Manager>, text: &str) -> Option<ServerMessage> {
    let Ok(ClientMessage::Request { id, op }) = serde_json::from_str::<ClientMessage>(text) else {
        return Some(ServerMessage::Reply {
            id: 0,
            result: OpResult::Error {
                error: ProtocolError::new(ErrorCode::InvalidRequest, "malformed control message"),
            },
        });
    };

    let result = match op {
        ClientOp::ListSessions => OpResult::Sessions {
            sessions: manager.list(),
        },
        ClientOp::ListHosts => OpResult::Hosts {
            hosts: manager.hosts(),
        },
        ClientOp::SaveLayout { tabs, panes } => match manager.save_layout(&tabs, &panes) {
            Ok(()) => OpResult::Ok,
            Err(error) => OpResult::Error { error },
        },
        ClientOp::LoadLayout => match manager.load_layout() {
            Ok((tabs, panes)) => OpResult::Layout { tabs, panes },
            Err(error) => OpResult::Error { error },
        },
        ClientOp::UpdateHost {
            host,
            name,
            group_name,
        } => match manager.update_host(&host, &name, group_name) {
            Ok(()) => OpResult::Ok,
            Err(error) => OpResult::Error { error },
        },
        ClientOp::DuplicateHost { host_id, new_id } => manager
            .duplicate_host(&host_id, &new_id)
            .map(|_| OpResult::Ok)
            .unwrap_or_else(|error| OpResult::Error { error }),
        ClientOp::DeleteSecret { id } => match manager.delete_secret(&id) {
            Ok(()) => OpResult::Ok,
            Err(error) => OpResult::Error { error },
        },
        ClientOp::DeleteHost { host_id } => match manager.delete_host(&host_id) {
            Ok(()) => OpResult::Ok,
            Err(error) => OpResult::Error { error },
        },
        ClientOp::SetSecret { id, secret } => match manager.store_secret(&id, &secret) {
            Ok(()) => OpResult::Ok,
            Err(error) => OpResult::Error { error },
        },
        ClientOp::CreateSession { host, pane_id, cwd } => {
            match manager.create(&host, pane_id, cwd) {
                Ok(summary) => OpResult::Session {
                    numeric: manager.numeric_id(&summary.session_id).unwrap_or_default(),
                    session_id: summary.session_id,
                    provider: summary.provider,
                    recreated: false,
                },
                Err(error) => OpResult::Error { error },
            }
        }
        ClientOp::Attach { session_id } => {
            match manager.list().iter().find(|s| s.session_id == session_id) {
                Some(summary) => OpResult::Session {
                    numeric: manager.numeric_id(&summary.session_id).unwrap_or_default(),
                    session_id: summary.session_id.clone(),
                    provider: summary.provider,
                    recreated: false,
                },
                None => OpResult::Error {
                    error: ProtocolError::new(ErrorCode::NotFound, "session not found"),
                },
            }
        }
        ClientOp::Detach { session_id } => {
            manager.detach(&session_id);
            OpResult::Ok
        }
        ClientOp::Destroy { session_id } => {
            manager.destroy(&session_id);
            OpResult::Ok
        }
        ClientOp::Resize {
            session_id,
            cols,
            rows,
        } => {
            manager.resize(&session_id, cols, rows);
            OpResult::Ok
        }
        ClientOp::Resync { session_id, .. } => {
            manager.replay(&session_id, REPLAY_LINES);
            OpResult::Ok
        }
        ClientOp::RestartDaemon => {
            // Detach first so remote tmux sessions outlive the daemon swap; the
            // supervisor restarts us and the GUI reattaches automatically.
            manager.shutdown_all();
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                std::process::exit(RESTART_EXIT_CODE);
            });
            OpResult::Ok
        }
    };
    Some(ServerMessage::Reply { id, result })
}

/// Validate the first client message. Returns the still-open socket on success.
pub async fn handshake(
    mut ws: WebSocketStream<TcpStream>,
    expected_token: &str,
    info: ServerInfo,
) -> Result<WebSocketStream<TcpStream>> {
    let first = ws
        .next()
        .await
        .context("connection closed before handshake")??;

    let accepted = match first {
        Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
            Ok(ClientMessage::Hello {
                protocol_version,
                token,
                ..
            }) => {
                if protocol_version != PROTOCOL_VERSION {
                    reject(&mut ws, "protocol_version", "protocol version mismatch").await?;
                    false
                } else if token != expected_token {
                    reject(&mut ws, "invalid_token", "invalid handshake token").await?;
                    false
                } else {
                    true
                }
            }
            _ => {
                reject(
                    &mut ws,
                    "invalid_request",
                    "handshake must start with hello",
                )
                .await?;
                false
            }
        },
        _ => {
            reject(&mut ws, "invalid_request", "handshake must be a text frame").await?;
            false
        }
    };

    if !accepted {
        anyhow::bail!("handshake rejected");
    }

    ws.send(Message::Text(serde_json::to_string(
        &ServerMessage::Hello { info },
    )?))
    .await?;
    Ok(ws)
}

async fn reject(ws: &mut WebSocketStream<TcpStream>, code: &str, reason: &str) -> Result<()> {
    ws.send(Message::Text(serde_json::to_string(
        &ServerMessage::Event {
            event: ServerEvent::DaemonShuttingDown {
                reason: format!("{code}: {reason}"),
            },
        },
    )?))
    .await?;
    ws.send(Message::Close(Some(CloseFrame {
        code: CloseCode::Policy,
        reason: reason.to_string().into(),
    })))
    .await?;
    Ok(())
}

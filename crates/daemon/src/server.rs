//! Loopback WebSocket server and the handshake gate.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, WebSocketStream};

use crate::paths::DaemonPaths;
use crate::token::{now_millis, write_daemon_file, DaemonFile};
use protocol::{ClientMessage, ServerInfo, ServerMessage, PROTOCOL_VERSION};

/// How long a client has to complete the handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// A running daemon listener.
pub struct DaemonServer {
    pub addr: SocketAddr,
    pub token: String,
    pub info: ServerInfo,
    task: JoinHandle<()>,
}

impl DaemonServer {
    /// Bind to an ephemeral loopback port, publish `daemon.json`, and start accepting.
    pub async fn start(paths: &DaemonPaths) -> Result<Self> {
        paths.ensure()?;

        // Stale files from a crashed run must never be trusted.
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

        let daemon_file = DaemonFile {
            pid: info.pid,
            port: addr.port(),
            token: token.clone(),
            protocol_version: PROTOCOL_VERSION,
            daemon_version: info.daemon_version.clone(),
            started_at: now_millis(),
        };
        write_daemon_file(&paths.daemon_file(), &daemon_file)?;

        let accept_token = token.clone();
        let accept_info = info.clone();
        let task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _peer)) => {
                        let token = accept_token.clone();
                        let info = accept_info.clone();
                        tokio::spawn(async move {
                            if let Err(error) = serve_connection(stream, token, info).await {
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
) -> Result<()> {
    let ws = accept_async(stream).await.context("websocket upgrade")?;
    let mut ws = match tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        handshake(ws, &expected_token, info.clone()),
    )
    .await
    {
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

    // Placeholder for the session command loop (task 2.3+). Keep the socket open and
    // drain control messages so the handshake test can observe a live connection.
    while let Some(message) = ws.next().await {
        match message? {
            Message::Close(_) => break,
            Message::Ping(payload) => ws.send(Message::Pong(payload)).await?,
            _ => {}
        }
    }
    Ok(())
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
            event: protocol::ServerEvent::DaemonShuttingDown {
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

//! Helper-backed pane over a single SSH exec channel.
//!
//! The channel runs `sentinel-helper proxy`, which forwards bytes to the helper's Unix
//! socket. Frames are decoded with `helper-proto`; terminal data is converted back into
//! the same `ChannelMsg` the exec path produces, so the session runner cannot tell the
//! two transports apart.

use anyhow::{Context, Result};
use async_trait::async_trait;
use helper_proto::{decode, encode, frame_len, Message};
use russh::client::Msg;
use russh::{Channel, ChannelMsg};
use tokio::sync::mpsc;

use crate::pane_channel::PaneChannel;

/// Path the deployment step (task 3.x) installs to.
pub const HELP_SOCKET_HINT: &str = "~/.local/share/sentinel/run/helper.sock";

fn proxy_command() -> String {
    // The helper lives next to its install dir; `proxy` finds the socket itself and can
    // be overridden for non-default locations.
    "\"$HOME/.local/share/sentinel/bin/sentinel-helper\" proxy".to_string()
}

pub struct FramePane {
    outgoing: mpsc::Sender<Vec<u8>>,
    incoming: mpsc::Receiver<ChannelMsg>,
    client_id: String,
    session_id: String,
    _reader: tokio::task::JoinHandle<()>,
}

impl FramePane {
    /// Start the proxy, create (or reattach to) `session_id`, and return a pane whose
    /// `next()` yields the replayed screen followed by live output.
    pub async fn open(
        client: &ssh::Connection,
        client_id: &str,
        session_id: &str,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let mut channel = client
            .exec(&proxy_command())
            .await
            .context("spawn helper proxy")?;

        // Handshake and session creation happen synchronously: the helper answers
        // `attach` with replay data *before* its reply, so we must consume frames here
        // before handing the channel to the reader task.
        let mut pending = Vec::new();
        request(
            &mut channel,
            &mut pending,
            Message::Hello {
                client_id: client_id.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        )
        .await?;

        let created = request(
            &mut channel,
            &mut pending,
            Message::Create {
                client_id: client_id.to_string(),
                session_id: session_id.to_string(),
                cwd: cwd.map(str::to_string),
                cols,
                rows,
            },
        )
        .await;

        // An existing session is not an error: fall through to attach.
        let _ = created;

        let (tx, rx) = mpsc::channel(256);
        for bytes in pending.drain(..) {
            let _ = tx.send(ChannelMsg::Data { data: bytes.into() }).await;
        }

        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
        let reader = tokio::spawn(async move {
            let mut channel = channel;
            let mut buffer: Vec<u8> = Vec::new();
            loop {
                if let Some(len) = frame_len(&buffer) {
                    if buffer.len() >= len {
                        let frame: Vec<u8> = buffer.drain(..len).collect();
                        match decode(&frame) {
                            Ok((Message::Data { seq, .. }, body)) => {
                                let _ = seq;
                                if tx
                                    .send(ChannelMsg::Data { data: body.into() })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                                continue;
                            }
                            Ok(_) => continue,
                            Err(_) => return,
                        }
                    }
                }
                tokio::select! {
                    message = channel.wait() => match message {
                        Some(ChannelMsg::Data { data }) => buffer.extend_from_slice(&data),
                        Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                            let _ = tx.send(ChannelMsg::Eof).await;
                            return;
                        }
                        Some(_) => {}
                    },
                    outgoing = out_rx.recv() => {
                        if let Some(frame) = outgoing {
                            if channel.data(frame.as_slice()).await.is_err() {
                                let _ = tx.send(ChannelMsg::Eof).await;
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            outgoing: out_tx,
            incoming: rx,
            client_id: client_id.to_string(),
            session_id: session_id.to_string(),
            _reader: reader,
        })
    }
}

async fn request(
    channel: &mut Channel<Msg>,
    pending: &mut Vec<Vec<u8>>,
    message: Message,
) -> Result<Option<String>> {
    channel.data(encode(&message, b"").as_slice()).await?;
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        if let Some(len) = frame_len(&buffer) {
            if buffer.len() >= len {
                let frame: Vec<u8> = buffer.drain(..len).collect();
                match decode(&frame)? {
                    (Message::Reply { ok, detail, .. }, _) => {
                        if ok {
                            return Ok(detail);
                        }
                        return Err(anyhow::anyhow!(
                            "helper rejected request: {}",
                            detail.unwrap_or_default()
                        ));
                    }
                    (Message::Data { .. }, body) => {
                        pending.push(body);
                        continue;
                    }
                    _ => continue,
                }
            }
        }
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) => buffer.extend_from_slice(&data),
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                return Err(anyhow::anyhow!("helper proxy closed during handshake"))
            }
            Some(_) => {}
        }
    }
}

#[async_trait]
impl PaneChannel for FramePane {
    async fn next(&mut self) -> Option<ChannelMsg> {
        self.incoming.recv().await
    }

    async fn write(&self, bytes: &[u8]) -> Result<()> {
        let frame = encode(
            &Message::Data {
                session_id: self.session_id.clone(),
                seq: 0,
                len: bytes.len() as u32,
            },
            bytes,
        );
        self.outgoing
            .send(frame)
            .await
            .map_err(|_| anyhow::anyhow!("helper proxy is gone"))?;
        Ok(())
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let frame = encode(
            &Message::Resize {
                client_id: self.client_id.clone(),
                session_id: self.session_id.clone(),
                cols,
                rows,
            },
            b"",
        );
        self.outgoing
            .send(frame)
            .await
            .map_err(|_| anyhow::anyhow!("helper proxy is gone"))?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        // Closing the channel is the reader task's job once it sees EOF; dropping the
        // pane drops the sender, which makes the task exit after the next read.
        Ok(())
    }
}

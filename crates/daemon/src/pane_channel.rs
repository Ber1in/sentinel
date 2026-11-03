//! Abstraction over the two ways a pane can be carried.
//!
//! - `ExecPane`: a plain SSH channel running a command (tmux attach, plain shell).
//! - `FramePane`: an SSH channel running `sentinel-helper proxy`, speaking frames.
//!
//! The session runner only needs these four operations, so the transport can be swapped
//! without touching the reconnect state machine.

use anyhow::Result;
use async_trait::async_trait;
use russh::client::Msg;
use russh::Channel;

/// One terminal pane, regardless of what carries it.
#[async_trait]
pub trait PaneChannel: Send {
    /// Next event from the pane; `None` when the pane is gone.
    async fn next(&mut self) -> Option<ChannelMsg>;
    /// Write user input.
    async fn write(&self, bytes: &[u8]) -> Result<()>;
    /// Tell the remote side about a new size.
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    /// Close the pane (detach).
    async fn close(&self) -> Result<()>;
}

pub use russh::ChannelMsg;

/// A pane backed by a single SSH exec channel.
pub struct ExecPane {
    channel: Channel<Msg>,
}

impl ExecPane {
    pub fn new(channel: Channel<Msg>) -> Self {
        Self { channel }
    }
}

#[async_trait]
impl PaneChannel for ExecPane {
    async fn next(&mut self) -> Option<ChannelMsg> {
        self.channel.wait().await
    }

    async fn write(&self, bytes: &[u8]) -> Result<()> {
        self.channel.data(bytes).await?;
        Ok(())
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.channel
            .window_change(cols as u32, rows as u32, 0, 0)
            .await?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        self.channel.close().await?;
        Ok(())
    }
}

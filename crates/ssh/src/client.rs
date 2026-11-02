//! Connection entry point: transport, keepalive, host-key policy, and channel opening.

use std::sync::Arc;
use std::time::Duration;

use russh::client::{Config, Handle};
use russh::keys::key::PublicKey;
use russh::Channel;

use crate::auth::Credentials;
use crate::error::SshError;
use crate::hostkey::HostKeyPolicy;

/// Keepalive settings mapped onto the SSH transport configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keepalive {
    pub interval: Duration,
    pub max_misses: usize,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            max_misses: 3,
        }
    }
}

/// Everything needed to establish one connection.
#[derive(Clone)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub credentials: Credentials,
    pub host_key_policy: HostKeyPolicy,
    pub keepalive: Keepalive,
    pub timeout: Duration,
}

/// Client-side SSH handler: verifies the server key before anything else happens.
pub struct ClientHandler {
    policy: HostKeyPolicy,
    host: String,
    port: u16,
}

#[async_trait::async_trait]
impl russh::client::Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        crate::hostkey::check(&self.policy, &self.host, self.port, key)?;
        Ok(true)
    }
}

/// A live, authenticated connection. Channels opened on it share one TCP connection.
pub struct Connection {
    handle: Handle<ClientHandler>,
    pub host: String,
    pub port: u16,
}

impl Connection {
    /// Connect and authenticate within `options.timeout`.
    pub async fn connect(options: &ConnectOptions) -> Result<Self, SshError> {
        let config = Arc::new(transport_config(options.keepalive));
        let handler = ClientHandler {
            policy: options.host_key_policy.clone(),
            host: options.host.clone(),
            port: options.port,
        };

        let attempt = async {
            let mut handle =
                russh::client::connect(config, (options.host.as_str(), options.port), handler)
                    .await?;
            crate::auth::authenticate(&mut handle, &options.user, &options.credentials).await?;
            Ok::<_, SshError>(handle)
        };

        let handle = tokio::time::timeout(options.timeout, attempt)
            .await
            .map_err(|_| {
                SshError::Timeout(format!("connect to {}:{}", options.host, options.port))
            })??;

        Ok(Self {
            handle,
            host: options.host.clone(),
            port: options.port,
        })
    }

    /// Open an interactive shell channel with a PTY of the given size.
    pub async fn open_shell(
        &self,
        term: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Channel<russh::client::Msg>, SshError> {
        let channel = self.handle.channel_open_session().await?;
        channel
            .request_pty(true, term, cols as u32, rows as u32, 0, 0, &[])
            .await?;
        channel.request_shell(true).await?;
        Ok(channel)
    }

    /// Open a PTY-backed channel and exec `command` in it. Used to attach tmux clients.
    pub async fn open_pty_exec(
        &self,
        command: &str,
        term: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Channel<russh::client::Msg>, SshError> {
        let channel = self.handle.channel_open_session().await?;
        channel
            .request_pty(true, term, cols as u32, rows as u32, 0, 0, &[])
            .await?;
        channel.exec(true, command).await?;
        Ok(channel)
    }

    /// Open a channel and run a one-shot command (tests, probes).
    pub async fn exec(&self, command: &str) -> Result<Channel<russh::client::Msg>, SshError> {
        let channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;
        Ok(channel)
    }

    /// Open an arbitrary subsystem channel (used by SFTP and by the reuse test).
    pub async fn open_subsystem(
        &self,
        name: &str,
    ) -> Result<Channel<russh::client::Msg>, SshError> {
        let channel = self.handle.channel_open_session().await?;
        channel.request_subsystem(true, name).await?;
        Ok(channel)
    }

    pub async fn disconnect(&self) -> Result<(), SshError> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await?;
        Ok(())
    }
}

/// Pure mapping so tests can assert keepalive wiring without a live server.
pub fn transport_config(keepalive: Keepalive) -> Config {
    Config {
        keepalive_interval: Some(keepalive.interval),
        keepalive_max: keepalive.max_misses,
        inactivity_timeout: None,
        ..Config::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_settings_land_in_the_transport_config() {
        let keepalive = Keepalive {
            interval: Duration::from_secs(15),
            max_misses: 5,
        };
        let config = transport_config(keepalive);
        assert_eq!(config.keepalive_interval, Some(Duration::from_secs(15)));
        assert_eq!(config.keepalive_max, 5);
        assert!(config.inactivity_timeout.is_none());
    }

    #[test]
    fn default_keepalive_is_thirty_seconds_and_three_misses() {
        let keepalive = Keepalive::default();
        assert_eq!(keepalive.interval, Duration::from_secs(30));
        assert_eq!(keepalive.max_misses, 3);
    }
}

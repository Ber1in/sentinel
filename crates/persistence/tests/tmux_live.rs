//! Live tmux tests against the throwaway sshd:
//!
//!   eval "$(./scripts/test-sshd.sh start)" && cargo test -p persistence --test tmux_live -- --nocapture

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use persistence::{
    CommandOutput, EnsureOutcome, PersistenceError, PersistenceProvider, RemoteExec, TmuxOptions,
    TmuxPersistence,
};
use russh::ChannelMsg;
use ssh::{ConnectOptions, Connection, Credentials, HostKeyPolicy, Keepalive};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

struct SshExec {
    connection: Arc<Connection>,
}

#[async_trait]
impl RemoteExec for SshExec {
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

async fn connect() -> Option<(Arc<Connection>, TmuxPersistence, String)> {
    let host = env("SSHTERM_TEST_HOST")?;
    let port: u16 = env("SSHTERM_TEST_PORT")?.parse().ok()?;
    let user = env("SSHTERM_TEST_USER")?;
    let password = env("SSHTERM_TEST_PASSWORD")?;

    let connection = Connection::connect(&ConnectOptions {
        host,
        port,
        user,
        credentials: Credentials::Password(password),
        host_key_policy: HostKeyPolicy::AcceptAny,
        keepalive: Keepalive::default(),
        timeout: Duration::from_secs(10),
    })
    .await
    .expect("connect");

    let provider = TmuxPersistence::new(TmuxOptions {
        socket: format!("sshtermtest-{}", std::process::id()),
        ..TmuxOptions::default()
    });
    let session = TmuxPersistence::make_session_name("testclient", "host", "pane");
    Some((Arc::new(connection), provider, session))
}

#[tokio::test]
async fn ensure_attaches_then_recreates_and_destroys() {
    let Some((connection, provider, session)) = connect().await else {
        return eprintln!("skipped: no test sshd");
    };
    let exec = SshExec { connection };

    let first = provider
        .ensure(&exec, &session, Some("/tmp"))
        .await
        .unwrap();
    assert_eq!(first, EnsureOutcome::Recreated);

    let second = provider
        .ensure(&exec, &session, Some("/tmp"))
        .await
        .unwrap();
    assert_eq!(
        second,
        EnsureOutcome::Attached,
        "second ensure must reattach"
    );

    // The session must survive the client that created it.
    let listed = exec
        .run(&format!(
            "tmux -L {} ls -F '#{{session_name}}'",
            provider.options().socket
        ))
        .await
        .unwrap();
    assert!(
        listed.stdout.contains(&session),
        "session missing: {:?}",
        listed
    );

    provider.destroy(&exec, &session).await.unwrap();
    let after = exec
        .run(&format!("tmux -L {} ls", provider.options().socket))
        .await
        .unwrap();
    assert!(!after.stdout.contains(&session), "session must be gone");
}

#[tokio::test]
async fn snapshot_replays_visible_output() {
    let Some((connection, provider, session)) = connect().await else {
        return eprintln!("skipped: no test sshd");
    };
    let exec = SshExec { connection };

    provider
        .ensure(&exec, &session, Some("/tmp"))
        .await
        .unwrap();
    exec.run(&format!(
        "tmux -L {} send-keys -t {} 'printf snapshot-marker' Enter",
        provider.options().socket,
        session
    ))
    .await
    .unwrap();
    // The detached shell needs a moment to start and draw; poll instead of sleeping.
    let mut snapshot = String::new();
    for _ in 0..40 {
        snapshot = provider
            .snapshot(&exec, &session, 200)
            .await
            .unwrap_or_else(|error| panic!("snapshot failed: {error}"));
        if snapshot.contains("snapshot-marker") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        snapshot.contains("snapshot-marker"),
        "snapshot should contain the marker: {snapshot:?}"
    );

    provider.destroy(&exec, &session).await.unwrap();
}

#[tokio::test]
async fn missing_tmux_is_reported() {
    let Some((connection, provider, session)) = connect().await else {
        return eprintln!("skipped: no test sshd");
    };
    let exec = SshExec { connection };
    // Hide tmux from PATH to simulate a host without it.
    let hidden = format!(
        "PATH=/nonexistent {} ",
        provider
            .probe_command()
            .replace("command -v tmux", "command -v tmux")
    );
    let output = exec.run(&hidden).await.unwrap();
    assert_eq!(output.stdout.trim(), "no");

    // The provider itself probes through the same command.
    let error = provider.ensure(&exec, &session, None).await;
    assert!(error.is_ok(), "the real host still has tmux");
}

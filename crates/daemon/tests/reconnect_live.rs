//! Live reconnect behaviour against the throwaway sshd:
//!
//!   eval "$(./scripts/test-sshd.sh start)" && cargo test -p daemon --test reconnect_live -- --nocapture --test-threads=1
//!
//! These tests stop and start the fixture sshd, so they must run serially.

use std::process::Command as Process;
use std::sync::Arc;
use std::time::Duration;

use daemon::fsm::ReconnectPolicy;
use daemon::runner::{Command, SessionRuntime, SessionSpec};
use persistence::{PersistenceProvider, TmuxOptions, TmuxPersistence};
use protocol::SessionState;
use ssh::{ConnectOptions, Credentials, HostKeyPolicy, Keepalive};
use tokio::sync::broadcast::error::RecvError;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn spec(session_id: &str, socket: &str, spill: &std::path::Path) -> Option<SessionSpec> {
    let host = env("SSHTERM_TEST_HOST")?;
    let port: u16 = env("SSHTERM_TEST_PORT")?.parse().ok()?;
    let user = env("SSHTERM_TEST_USER")?;
    let password = env("SSHTERM_TEST_PASSWORD")?;

    Some(SessionSpec {
        session_id: session_id.to_string(),
        namespace: "testns".into(),
        host_id: "testhost".into(),
        pane_id: format!("pane-{session_id}"),
        connect: ConnectOptions {
            host,
            port,
            user,
            credentials: Credentials::Password(password),
            host_key_policy: HostKeyPolicy::AcceptAny,
            keepalive: Keepalive::default(),
            timeout: Duration::from_secs(10),
        },
        provider: Arc::new(TmuxPersistence::new(TmuxOptions {
            socket: socket.into(),
            ..TmuxOptions::default()
        })),
        cwd: Some("/tmp".into()),
        scrollback_lines: 10_000,
        spill_dir: Some(spill.to_path_buf()),
        spill_max_bytes: 8 * 1024 * 1024,
        policy: ReconnectPolicy {
            initial: Duration::from_millis(200),
            max: Duration::from_secs(2),
        },
        term: "xterm-256color".into(),
        cols: 100,
        rows: 30,
    })
}

fn script(action: &str) {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Process::new(root.join("scripts/test-sshd.sh"))
        .arg(action)
        .output()
        .expect("run fixture script");
    assert!(
        output.status.success(),
        "fixture {action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn wait_state(runtime: &mut SessionRuntime, target: SessionState, secs: u64) -> bool {
    let receiver = &mut runtime.events.state;
    if *receiver.borrow() == target {
        return true;
    }
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            if receiver.changed().await.is_err() {
                return false;
            }
            if *receiver.borrow() == target {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false)
}

async fn collect_until(
    receiver: &mut tokio::sync::broadcast::Receiver<daemon::runner::OutputChunk>,
    needle: &str,
    secs: u64,
) -> String {
    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if collected.contains(needle) {
            return collected;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return collected;
        }
        match tokio::time::timeout(remaining, receiver.recv()).await {
            Ok(Ok(chunk)) => collected.push_str(&String::from_utf8_lossy(&chunk.bytes)),
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) | Err(_) => return collected,
        }
    }
}

#[tokio::test]
async fn reconnects_after_the_server_drops_and_replays_history() {
    let Some(spec) = spec(
        "s1",
        &format!("sshtermtest-r-{}", std::process::id()),
        tempdir().as_path(),
    ) else {
        return eprintln!("skipped: no test sshd");
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let mut runtime = SessionRuntime::spawn(spec);
    let mut output = runtime.events.output.subscribe();

    assert!(
        wait_state(&mut runtime, SessionState::Connected, 30).await,
        "initial connect"
    );

    runtime
        .send(Command::Input(b"echo marker-one\n".to_vec()))
        .await;
    let seen = collect_until(&mut output, "marker-one", 15).await;
    assert!(
        seen.contains("marker-one"),
        "initial echo missing: {seen:?}"
    );

    script("drop");
    assert!(
        wait_state(&mut runtime, SessionState::Reconnecting, 20).await,
        "link loss must move to reconnecting"
    );
    assert!(
        wait_state(&mut runtime, SessionState::Connected, 40).await,
        "must reconnect on its own"
    );

    runtime
        .send(Command::Input(b"echo marker-two\n".to_vec()))
        .await;
    let seen = collect_until(&mut output, "marker-two", 20).await;
    assert!(
        seen.contains("marker-two"),
        "post-reconnect echo missing: {seen:?}"
    );

    let replay = String::from_utf8_lossy(&runtime.replay(2_000)).into_owned();
    assert!(
        replay.contains("marker-one"),
        "history must survive the reconnect"
    );
    let occurrences = replay.matches("marker-one").count();
    assert!(
        (1..=2).contains(&occurrences),
        "history must contain the pre-disconnect output (snapshot may duplicate the visible screen once), got {occurrences}: {replay:?}"
    );

    runtime.send(Command::Destroy).await;
}

#[tokio::test]
async fn a_vanished_remote_session_is_recreated_and_reported() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let socket = format!("sshtermtest-v-{}", std::process::id());
    let Some(spec) = spec("s2", &socket, tempdir().as_path()) else {
        return eprintln!("skipped: no test sshd");
    };
    let mut runtime = SessionRuntime::spawn(spec);
    assert!(
        wait_state(&mut runtime, SessionState::Connected, 30).await,
        "initial connect"
    );
    runtime.clear_recreated();

    // Take the listener down first so the runner cannot reattach while we remove the
    // remote session, then sever the live connections.
    script("stop");
    script("drop");
    assert!(wait_state(&mut runtime, SessionState::Reconnecting, 20).await);

    // The remote session disappears while we are disconnected. The tmux socket lives per
    // uid, so this must run as the test user rather than as the test runner.
    let user = env("SSHTERM_TEST_USER").unwrap();
    let session = TmuxPersistence::make_session_name("testns", "testhost", "pane-s2");
    let killed = Process::new("runuser")
        .args([
            "-u",
            &user,
            "--",
            "tmux",
            "-L",
            &socket,
            "kill-session",
            "-t",
            &session,
        ])
        .status();
    assert!(
        killed.map(|status| status.success()).unwrap_or(false),
        "the remote session must exist before we remove it"
    );
    assert!(
        !*runtime.events.recreated.borrow(),
        "flag must be clear before the reconnect"
    );

    script("start");
    assert!(
        wait_state(&mut runtime, SessionState::Connected, 40).await,
        "must reconnect after the server comes back"
    );
    assert!(
        *runtime.events.recreated.borrow(),
        "recreating the remote session must be reported to the GUI"
    );

    runtime.send(Command::Destroy).await;
}

fn tempdir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("sshterm-spill-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

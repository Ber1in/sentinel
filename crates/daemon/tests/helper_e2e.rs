//! End-to-end helper path: deploy over SSH, then run a session through `FramePane`.
//!
//! Requires the local sshd fixture and a built musl helper:
//!   eval "$(./scripts/test-sshd.sh start)" && cargo test -p daemon --test helper_e2e

use std::time::Duration;

use daemon::deploy;
use daemon::frame_pane::FramePane;
use daemon::pane_channel::{ChannelMsg, PaneChannel};
use ssh::{ConnectOptions, Connection, Credentials, HostKeyPolicy, Keepalive};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[tokio::test]
async fn deploys_and_runs_a_session_through_the_helper() {
    let Some(host) = env("SSHTERM_TEST_HOST") else {
        return eprintln!("skipped: no test sshd");
    };
    let port: u16 = env("SSHTERM_TEST_PORT").unwrap().parse().unwrap();
    let user = env("SSHTERM_TEST_USER").unwrap();
    let password = env("SSHTERM_TEST_PASSWORD").unwrap();

    let helper_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/helpers");
    if !helper_dir.exists() {
        return eprintln!("skipped: build dist/helpers first");
    }

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

    let probe = deploy::ensure(&connection, &helper_dir)
        .await
        .expect("deploy helper");
    assert!(probe.ok(), "probe should be clean: {probe:?}");

    let mut pane = FramePane::open(
        &connection,
        "e2e-client",
        "e2e-session",
        Some("/tmp"),
        100,
        30,
    )
    .await
    .expect("open helper pane");

    pane.write(b"echo frame-marker\n").await.unwrap();

    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline && !output.contains("frame-marker") {
        match tokio::time::timeout(Duration::from_secs(2), pane.next()).await {
            Ok(Some(ChannelMsg::Data { data })) => {
                output.push_str(&String::from_utf8_lossy(&data));
            }
            Ok(Some(ChannelMsg::Eof)) | Ok(None) => break,
            Ok(Some(_)) => {}
            Err(_) => continue,
        }
    }
    assert!(output.contains("frame-marker"), "output was: {output:?}");

    // A second ensure must not re-deploy: it returns as soon as `version` answers.
    deploy::ensure(&connection, &helper_dir)
        .await
        .expect("idempotent ensure");

    let _ = daemon::deploy::execute(&connection, "pkill -f 'sentinel-helper serve' || true").await;
    let _ = daemon::deploy::execute(&connection, "rm -rf ~/.local/share/sentinel").await;
    connection.disconnect().await.unwrap();
}

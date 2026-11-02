//! Integration tests against a real sshd. Run `./scripts/test-sshd.sh start` and pass the
//! printed environment, e.g.:
//!
//!   eval "$(./scripts/test-sshd.sh start)" && cargo test -p ssh --test live -- --nocapture --test-threads=1
//!
//! Without the environment these tests skip instead of failing, so the default suite
//! stays hermetic.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use protocol::ErrorCode;
use russh::ChannelMsg;
use ssh::{ConnectOptions, Connection, Credentials, HostKeyPolicy, Keepalive, KnownHosts};

/// Tests share one sshd and one TCP table, so they must not overlap.
static SERIAL: Mutex<()> = Mutex::new(());

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

struct Target {
    host: String,
    port: u16,
    user: String,
    password: String,
    key: PathBuf,
}

fn target() -> Option<Target> {
    Some(Target {
        host: env("SSHTERM_TEST_HOST")?,
        port: env("SSHTERM_TEST_PORT")?.parse().ok()?,
        user: env("SSHTERM_TEST_USER")?,
        password: env("SSHTERM_TEST_PASSWORD")?,
        key: env("SSHTERM_TEST_KEY")?.into(),
    })
}

fn options(target: &Target, credentials: Credentials) -> ConnectOptions {
    ConnectOptions {
        host: target.host.clone(),
        port: target.port,
        user: target.user.clone(),
        credentials,
        host_key_policy: HostKeyPolicy::AcceptAny,
        keepalive: Keepalive::default(),
        timeout: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn password_auth_connects() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    let credentials = Credentials::Password(target.password.clone());
    let connection = Connection::connect(&options(&target, credentials))
        .await
        .expect("password auth should succeed");
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn key_auth_connects() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    let credentials = Credentials::Key {
        path: Some(target.key.clone()),
        pem: None,
        passphrase: None,
    };
    let connection = Connection::connect(&options(&target, credentials))
        .await
        .expect("key auth should succeed");
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn wrong_password_is_an_auth_failure() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    let credentials = Credentials::Password("definitely-not-the-password".into());
    let error = Connection::connect(&options(&target, credentials))
        .await
        .err()
        .expect("wrong password must fail");
    assert_eq!(error.code(), ErrorCode::AuthFailed);
    assert!(!error.retryable());
}

#[tokio::test]
async fn trust_on_first_use_then_reject_a_changed_key() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("known_hosts");
    let store = Arc::new(Mutex::new(KnownHosts::at(&store_path).unwrap()));

    let mut first = options(&target, Credentials::Password(target.password.clone()));
    first.host_key_policy = HostKeyPolicy::TrustOnFirstUse(store.clone());
    let connection = Connection::connect(&first)
        .await
        .expect("first use is accepted");
    connection.disconnect().await.unwrap();
    assert!(KnownHosts::at(&store_path)
        .unwrap()
        .known(&target.host, target.port)
        .is_some());

    // Pretend the server presented a different key earlier.
    std::fs::write(
        &store_path,
        format!("{}:{} SHA256:not-the-real-key\n", target.host, target.port),
    )
    .unwrap();
    let poisoned = KnownHosts::at(&store_path).unwrap();

    let mut second = options(&target, Credentials::Password(target.password.clone()));
    second.host_key_policy = HostKeyPolicy::TrustOnFirstUse(Arc::new(Mutex::new(poisoned)));
    let error = Connection::connect(&second)
        .await
        .err()
        .expect("changed host key must be rejected");
    assert_eq!(error.code(), ErrorCode::HostKeyMismatch);
}

fn established_connections_to(port: u16) -> usize {
    let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else {
        return 0;
    };
    table
        .lines()
        .skip(1)
        .filter(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 4 || fields[3] != "01" {
                return false;
            }
            fields[2]
                .rsplit_once(':')
                .and_then(|(_, port)| u16::from_str_radix(port, 16).ok())
                == Some(port)
        })
        .count()
}

#[tokio::test]
async fn two_channels_share_one_tcp_connection() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    let before = established_connections_to(target.port);
    let connection = Connection::connect(&options(
        &target,
        Credentials::Password(target.password.clone()),
    ))
    .await
    .unwrap();

    let _shell = connection
        .open_shell("xterm-256color", 80, 24)
        .await
        .unwrap();
    let _sftp = connection.open_subsystem("sftp").await.unwrap();

    let after = established_connections_to(target.port);
    assert_eq!(
        after,
        before + 1,
        "both channels must share a single TCP connection (before={before}, after={after})"
    );
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn throughput_reaches_the_baseline() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    const BYTES: usize = 10 * 1024 * 1024;

    let connection = Connection::connect(&options(
        &target,
        Credentials::Password(target.password.clone()),
    ))
    .await
    .unwrap();
    let mut channel = connection
        .exec(&format!("head -c {BYTES} /dev/zero"))
        .await
        .unwrap();

    let started = Instant::now();
    let mut received = 0usize;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => received += data.len(),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    let elapsed = started.elapsed();
    let mib_per_second = (received as f64 / 1024.0 / 1024.0) / elapsed.as_secs_f64();

    println!(
        "throughput: {:.1} MiB/s ({} bytes in {:?})",
        mib_per_second, received, elapsed
    );
    assert_eq!(received, BYTES, "all bytes must arrive");
    if cfg!(debug_assertions) {
        // Unoptimised russh crypto is roughly two orders of magnitude slower; the
        // baseline is only meaningful for release builds.
        println!("note: debug build, baseline not enforced");
    } else {
        assert!(
            mib_per_second >= 10.0,
            "baseline is 10 MiB/s, measured {mib_per_second:.1}"
        );
    }
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn keepalive_holds_an_idle_connection_open() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };

    let mut options = options(&target, Credentials::Password(target.password.clone()));
    options.keepalive = Keepalive {
        interval: Duration::from_secs(1),
        max_misses: 3,
    };
    let connection = Connection::connect(&options).await.unwrap();

    // Idle long enough for several keepalives to be sent and answered.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let mut channel = connection.exec("echo alive").await.unwrap();
    let mut output = String::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.push_str(&String::from_utf8_lossy(&data)),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    assert!(
        output.contains("alive"),
        "idle connection must still work: {output:?}"
    );
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn agent_auth_connects() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    if env("SSH_AUTH_SOCK").is_none() {
        return eprintln!("skipped: no ssh-agent from the fixture");
    }
    let connection = Connection::connect(&options(&target, Credentials::Agent))
        .await
        .expect("agent auth should succeed");
    connection.disconnect().await.unwrap();
}

#[tokio::test]
async fn keyboard_interactive_connects() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = target() else {
        return eprintln!("skipped: no test sshd");
    };
    let password = target.password.clone();
    let responder: ssh::KiResponder = Arc::new(move |request: &ssh::KiRequest| {
        // Servers may send empty rounds first; answering with an empty list is valid.
        vec![password.clone(); request.prompts.len()]
    });
    let connection = Connection::connect(&options(
        &target,
        Credentials::KeyboardInteractive { responder },
    ))
    .await
    .expect("keyboard-interactive auth should succeed");
    connection.disconnect().await.unwrap();
}

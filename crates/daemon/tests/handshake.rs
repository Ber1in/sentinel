use std::time::Duration;

use daemon::config::Config;
use daemon::credentials::MemoryStore;
use daemon::{DaemonPaths, DaemonServer, Manager};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;

async fn start(paths: &DaemonPaths) -> DaemonServer {
    let manager = Manager::new(
        Arc::new(MemoryStore::new()),
        Config::default(),
        ssh::KnownHosts::in_memory(),
        store::Store::in_memory().unwrap(),
    );
    DaemonServer::start(paths, manager).await.unwrap()
}

async fn client(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    ws
}

#[tokio::test]
async fn valid_token_receives_server_hello() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DaemonPaths::from_root(dir.path());
    let server = start(&paths).await;

    let mut ws = client(&server.url()).await;
    let hello = ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: server.token.clone(),
        client_version: "test".into(),
    };
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
        .unwrap();

    let reply = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("reply in time")
        .expect("message")
        .expect("ok");
    let Message::Text(text) = reply else {
        panic!("expected text, got {reply:?}")
    };
    match serde_json::from_str::<ServerMessage>(&text).unwrap() {
        ServerMessage::Hello { info } => {
            assert_eq!(info.protocol_version, PROTOCOL_VERSION);
            assert_eq!(info.pid, std::process::id());
        }
        other => panic!("expected hello, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_token_is_closed_without_hello() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DaemonPaths::from_root(dir.path());
    let server = start(&paths).await;

    let mut ws = client(&server.url()).await;
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            token: "not-the-token".into(),
            client_version: "test".into(),
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    let mut saw_hello = false;
    for _ in 0..4 {
        match tokio::time::timeout(Duration::from_secs(2), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if matches!(
                    serde_json::from_str::<ServerMessage>(&text),
                    Ok(ServerMessage::Hello { .. })
                ) {
                    saw_hello = true;
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => break,
            _ => break,
        }
    }
    assert!(!saw_hello, "bad token must never receive a hello");
}

#[tokio::test]
async fn mismatched_protocol_version_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DaemonPaths::from_root(dir.path());
    let server = start(&paths).await;

    let mut ws = client(&server.url()).await;
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION + 99,
            token: server.token.clone(),
            client_version: "test".into(),
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    let mut saw_hello = false;
    for _ in 0..4 {
        match tokio::time::timeout(Duration::from_secs(2), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if matches!(
                    serde_json::from_str::<ServerMessage>(&text),
                    Ok(ServerMessage::Hello { .. })
                ) {
                    saw_hello = true;
                }
            }
            _ => break,
        }
    }
    assert!(!saw_hello, "version mismatch must not receive a hello");
}

#[tokio::test]
async fn daemon_file_is_written_with_the_live_port() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DaemonPaths::from_root(dir.path());
    let server = start(&paths).await;

    let published = daemon::token::read_daemon_file(&paths.daemon_file()).unwrap();
    assert_eq!(published.port, server.addr.port());
    assert_eq!(published.token, server.token);
    assert_eq!(published.pid, std::process::id());
    assert_eq!(published.protocol_version, PROTOCOL_VERSION);
}

#[tokio::test]
async fn list_sessions_replies_with_an_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DaemonPaths::from_root(dir.path());
    let server = start(&paths).await;

    let mut ws = client(&server.url()).await;
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            token: server.token.clone(),
            client_version: "test".into(),
        })
        .unwrap(),
    ))
    .await
    .unwrap();
    let _hello = ws.next().await.unwrap().unwrap();

    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Request {
            id: 7,
            op: protocol::ClientOp::ListSessions,
        })
        .unwrap(),
    ))
    .await
    .unwrap();

    let reply = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("reply in time")
        .unwrap()
        .unwrap();
    let Message::Text(text) = reply else {
        panic!("expected text")
    };
    match serde_json::from_str::<ServerMessage>(&text).unwrap() {
        ServerMessage::Reply { id, result } => {
            assert_eq!(id, 7);
            match result {
                protocol::OpResult::Sessions { sessions } => assert!(sessions.is_empty()),
                other => panic!("unexpected result {other:?}"),
            }
        }
        other => panic!("unexpected message {other:?}"),
    }
}

#[tokio::test]
async fn create_session_streams_terminal_output() {
    let Some(host) = std::env::var("SSHTERM_TEST_HOST")
        .ok()
        .filter(|v| !v.is_empty())
    else {
        return eprintln!("skipped: no test sshd");
    };
    let port: u16 = std::env::var("SSHTERM_TEST_PORT").unwrap().parse().unwrap();
    let user = std::env::var("SSHTERM_TEST_USER").unwrap();
    let password = std::env::var("SSHTERM_TEST_PASSWORD").unwrap();

    let dir = tempfile::tempdir().unwrap();
    let paths = DaemonPaths::from_root(dir.path());
    let secrets = Arc::new(MemoryStore::new());
    use daemon::credentials::SecretStore;
    secrets.set("host-1:password", &password).unwrap();
    let manager = Manager::new(
        secrets,
        Config::default(),
        ssh::KnownHosts::in_memory(),
        store::Store::in_memory().unwrap(),
    );
    let server = DaemonServer::start(&paths, manager).await.unwrap();

    let mut ws = client(&server.url()).await;
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            token: server.token.clone(),
            client_version: "test".into(),
        })
        .unwrap(),
    ))
    .await
    .unwrap();
    let _hello = ws.next().await.unwrap().unwrap();

    let create = protocol::ClientOp::CreateSession {
        host: protocol::HostSpec {
            host_id: "host-1".into(),
            hostname: host,
            port,
            username: user,
            auth: protocol::AuthMethod::Password {
                credential_id: "host-1:password".into(),
            },
        },
        pane_id: "pane-1".into(),
        cwd: Some("/tmp".into()),
    };
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Request { id: 1, op: create }).unwrap(),
    ))
    .await
    .unwrap();

    let mut session_numeric = None;
    let mut reply_ok = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        let message = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("frame in time")
            .expect("frame")
            .expect("ok");
        match message {
            Message::Text(text) => {
                if let Ok(ServerMessage::Reply { id, result }) = serde_json::from_str(&text) {
                    if id == 1 {
                        assert!(
                            matches!(result, protocol::OpResult::Session { .. }),
                            "create_session failed: {result:?}"
                        );
                        reply_ok = true;
                    }
                }
            }
            _ => {}
        }
        if reply_ok {
            break;
        }
    }
    assert!(reply_ok, "create_session must succeed");

    // Wait for output to start, learning the numeric id from the first binary frame.
    let mut text = String::new();
    while session_numeric.is_none() {
        let message = tokio::time::timeout(Duration::from_secs(20), ws.next())
            .await
            .expect("output in time")
            .expect("frame")
            .expect("ok");
        if let Message::Binary(frame) = message {
            let (_, numeric, _, payload) = protocol::decode_data_frame(&frame).unwrap();
            session_numeric = Some(numeric);
            text.push_str(&String::from_utf8_lossy(payload));
        }
    }
    let numeric = session_numeric.unwrap();

    ws.send(Message::Binary(protocol::encode_data_frame(
        protocol::FrameKind::Input,
        numeric,
        0,
        b"echo stream-marker\n",
    )))
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline && !text.contains("stream-marker") {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(Message::Binary(frame)))) => {
                let (_, _, _, payload) = protocol::decode_data_frame(&frame).unwrap();
                text.push_str(&String::from_utf8_lossy(payload));
            }
            _ => break,
        }
    }
    assert!(
        text.contains("stream-marker"),
        "terminal output missing: {text:?}"
    );

    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Request {
            id: 2,
            op: protocol::ClientOp::Destroy {
                session_id: "host-1:pane-1".into(),
            },
        })
        .unwrap(),
    ))
    .await
    .unwrap();
}

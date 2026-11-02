use std::time::Duration;

use daemon::{DaemonPaths, DaemonServer};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::Message;

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
    let server = DaemonServer::start(&paths).await.unwrap();

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
    let server = DaemonServer::start(&paths).await.unwrap();

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
    let server = DaemonServer::start(&paths).await.unwrap();

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
    let server = DaemonServer::start(&paths).await.unwrap();

    let published = daemon::token::read_daemon_file(&paths.daemon_file()).unwrap();
    assert_eq!(published.port, server.addr.port());
    assert_eq!(published.token, server.token);
    assert_eq!(published.pid, std::process::id());
    assert_eq!(published.protocol_version, PROTOCOL_VERSION);
}

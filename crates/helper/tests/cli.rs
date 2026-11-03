//! End-to-end tests for the `sentinel-helper` binary.
//!
//! Everything here is hermetic: each test gets its own temporary `HOME`, socket
//! and lock file, and the PTY tests run `/bin/sh` locally. No sshd required.

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_sentinel-helper");
const WAIT: Duration = Duration::from_secs(15);

fn wait_until(mut predicate: impl FnMut() -> bool, timeout: Duration, what: &str) {
    let deadline = Instant::now() + timeout;
    loop {
        if predicate() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// One running `serve` process plus its private state directory.
struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
    socket: PathBuf,
}

impl Daemon {
    fn start(idle_secs: u64) -> Daemon {
        Self::start_with(idle_secs, &[])
    }

    fn start_with(idle_secs: u64, extra_env: &[(&str, &str)]) -> Daemon {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("run").join("helper.sock");
        let log = File::create(dir.path().join("serve.log")).unwrap();
        let mut command = Command::new(BIN);
        command
            .arg("serve")
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .env("SENTINEL_HELPER_SOCKET", &socket)
            .env("SENTINEL_HELPER_IDLE_SECS", idle_secs.to_string())
            .env("SHELL", "/bin/sh")
            .env("SENTINEL_HELPER_LOGIN_SHELL", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log));
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let child = command.spawn().expect("spawn sentinel-helper serve");
        let mut daemon = Daemon { child, dir, socket };
        daemon.wait_ready();
        daemon
    }

    fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        command
            .arg("serve")
            .current_dir(self.dir.path())
            .env("HOME", self.dir.path())
            .env("SENTINEL_HELPER_SOCKET", &self.socket)
            .env("SENTINEL_HELPER_IDLE_SECS", "600")
            .env("SHELL", "/bin/sh")
            .env("SENTINEL_HELPER_LOGIN_SHELL", "0");
        command
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(stream) = UnixStream::connect(&self.socket) {
                drop(stream);
                return;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "daemon exited before listening ({status}). log:\n{}",
                    self.log()
                );
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!(
                    "daemon did not listen on {} within {WAIT:?}. log:\n{}",
                    self.socket.display(),
                    self.log()
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("serve.log")).unwrap_or_default()
    }

    fn connect(&self) -> Client {
        Client::connect(&self.socket)
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "daemon did not exit within {timeout:?}. log:\n{}",
                self.log()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Minimal frame client; mirrors `frame.rs` without importing crate internals.
struct Client {
    stream: UnixStream,
    data: Vec<u8>,
}

impl Client {
    fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).expect("connect to helper socket");
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        Self {
            stream,
            data: Vec::new(),
        }
    }

    fn send_value(&mut self, message: &Value, body: &[u8]) {
        let header = serde_json::to_vec(message).unwrap();
        let mut frame = Vec::with_capacity(4 + header.len() + body.len());
        frame.extend_from_slice(&((header.len() + body.len()) as u32).to_be_bytes());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(body);
        self.stream.write_all(&frame).unwrap();
        self.stream.flush().unwrap();
    }

    fn recv_value(&mut self) -> (Value, Vec<u8>) {
        let mut len = [0u8; 4];
        self.stream.read_exact(&mut len).expect("frame length");
        let total = u32::from_be_bytes(len) as usize;
        let mut payload = vec![0u8; total];
        self.stream.read_exact(&mut payload).expect("frame payload");
        let mut stream = serde_json::Deserializer::from_slice(&payload).into_iter::<Value>();
        let value = stream.next().expect("header").expect("json header");
        let header_len = stream.byte_offset();
        (value, payload[header_len..].to_vec())
    }

    /// Read until a `reply`, collecting `data` bodies on the way.
    fn reply(&mut self) -> (bool, Option<String>) {
        loop {
            let (value, body) = self.recv_value();
            match value["kind"].as_str() {
                Some("reply") => {
                    return (
                        value["ok"].as_bool().expect("reply.ok"),
                        value["detail"].as_str().map(str::to_string),
                    )
                }
                Some("data") => self.data.extend_from_slice(&body),
                Some("event") => {}
                other => panic!("unexpected frame kind {other:?}: {value}"),
            }
        }
    }

    fn reply_ok(&mut self) -> Option<String> {
        let (ok, detail) = self.reply();
        assert!(ok, "expected ok reply, got: {detail:?}");
        detail
    }

    fn read_data_until(&mut self, needle: &str) {
        let deadline = Instant::now() + WAIT;
        loop {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; got {:?}",
                self.text()
            );
            let (value, body) = self.recv_value();
            if value["kind"] == "data" {
                self.data.extend_from_slice(&body);
                if self.text().contains(needle) {
                    return;
                }
            }
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }

    fn hello(&mut self, client_id: &str) {
        self.send_value(
            &json!({"kind":"hello","client_id":client_id,"version":"0.1.0"}),
            b"",
        );
        let detail = self.reply_ok().unwrap_or_default();
        assert!(
            detail.contains("helper"),
            "unexpected hello detail: {detail}"
        );
    }

    fn list(&mut self, client_id: &str) -> Vec<Value> {
        self.send_value(&json!({"kind":"list","client_id":client_id}), b"");
        let detail = self.reply_ok().expect("list detail");
        serde_json::from_str(&detail).expect("list detail is json")
    }

    fn create(&mut self, client_id: &str, session_id: &str, cwd: Option<&Path>) -> Value {
        self.send_value(
            &json!({
                "kind":"create",
                "client_id":client_id,
                "session_id":session_id,
                "cwd":cwd.map(|path| path.to_string_lossy().into_owned()),
                "cols":100,
                "rows":40
            }),
            b"",
        );
        let detail = self.reply_ok().expect("create detail");
        serde_json::from_str(&detail).expect("create detail is json")
    }

    fn input(&mut self, session_id: &str, payload: &str) {
        self.send_value(
            &json!({
                "kind":"data",
                "session_id":session_id,
                "seq":0,
                "len":payload.len()
            }),
            payload.as_bytes(),
        );
    }

    fn attach(&mut self, client_id: &str, session_id: &str) -> (bool, Option<String>) {
        self.send_value(
            &json!({"kind":"attach","client_id":client_id,"session_id":session_id}),
            b"",
        );
        self.reply()
    }

    fn resize(
        &mut self,
        client_id: &str,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> (bool, Option<String>) {
        self.send_value(
            &json!({
                "kind":"resize",
                "client_id":client_id,
                "session_id":session_id,
                "cols":cols,
                "rows":rows
            }),
            b"",
        );
        self.reply()
    }
}

fn read_raw_frame<R: Read>(stream: &mut R) -> Vec<u8> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).expect("frame length");
    let total = u32::from_be_bytes(len) as usize;
    let mut frame = len.to_vec();
    frame.resize(4 + total, 0);
    stream.read_exact(&mut frame[4..]).expect("frame payload");
    frame
}

fn raw_frame(message: &Value) -> Vec<u8> {
    let header = serde_json::to_vec(message).unwrap();
    let mut frame = Vec::new();
    frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
    frame.extend_from_slice(&header);
    frame
}

#[test]
fn version_and_usage_exit_codes() {
    let output = Command::new(BIN).arg("version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        env!("CARGO_PKG_VERSION")
    );

    let output = Command::new(BIN).arg("nonsense").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn proxy_bridges_frames_and_exits_zero_when_the_socket_closes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("helper.sock");
    let listener = UnixListener::bind(&path).unwrap();

    let echo = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let frame = read_raw_frame(&mut stream);
        stream.write_all(&frame).unwrap();
        // Keep the connection until the client has received the echo, then close.
        thread::sleep(Duration::from_millis(100));
    });

    let mut child = Command::new(BIN)
        .arg("proxy")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    let hello = json!({"kind":"hello","client_id":"c1","version":"0.1.0"});
    let sent = raw_frame(&hello);
    stdin.write_all(&sent).unwrap();
    stdin.flush().unwrap();

    let echoed = read_raw_frame(&mut stdout);
    assert_eq!(echoed, sent, "proxy must be byte transparent");

    // The fake server drops its socket; the proxy must exit 0 on its own.
    echo.join().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(0),
        "proxy should exit 0 on socket close"
    );
    drop(stdin);
}

#[test]
fn proxy_drains_replies_after_stdin_closes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("helper.sock");
    let listener = UnixListener::bind(&path).unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let frame = read_raw_frame(&mut stream);
        assert!(!frame.is_empty());
        // Answer, then wait for the proxy to propagate the stdin EOF.
        let reply = raw_frame(&json!({"kind":"reply","id":1,"ok":true,"detail":"bye"}));
        stream.write_all(&reply).unwrap();
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "unexpected extra bytes: {rest:?}");
    });

    let mut child = Command::new(BIN)
        .arg("proxy")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    stdin
        .write_all(&raw_frame(&json!({"kind":"list","client_id":"c1"})))
        .unwrap();
    stdin.flush().unwrap();
    drop(stdin); // EOF on stdin: proxy must half-close and keep reading

    let reply = read_raw_frame(&mut stdout);
    let value: Value = serde_json::from_slice(&reply[4..]).unwrap();
    assert_eq!(value["detail"], "bye");

    server.join().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn proxy_reports_a_missing_socket() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(BIN)
        .arg("proxy")
        .arg(dir.path().join("nope.sock"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("nope.sock"));
}

#[test]
fn socket_and_run_directory_are_private() {
    let daemon = Daemon::start(600);
    let socket_mode = std::fs::metadata(&daemon.socket)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600, "socket must be 0600");
    let dir_mode = std::fs::metadata(daemon.socket.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "run directory must be 0700");
    assert!(daemon.socket.exists());
}

#[test]
fn proxy_talks_to_a_real_daemon() {
    let daemon = Daemon::start(600);
    let mut child = Command::new(BIN)
        .arg("proxy")
        .arg(&daemon.socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    // Same bytes a client would push over an SSH exec channel.
    stdin
        .write_all(&raw_frame(&json!({
            "kind":"hello",
            "client_id":"proxy-user",
            "version":"0.1.0"
        })))
        .unwrap();
    stdin.flush().unwrap();

    let reply = read_raw_frame(&mut stdout);
    let value: Value = serde_json::from_slice(&reply[4..]).unwrap();
    assert_eq!(value["kind"], "reply");
    assert_eq!(value["ok"], true);
    assert!(value["detail"].as_str().unwrap().contains("helper"));

    // Closing stdin makes the proxy half-close, the daemon answer, and the proxy
    // exit cleanly.
    drop(stdin);
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    // The daemon is still running for the next client.
    assert!(daemon.socket.exists());
}

#[test]
fn serve_end_to_end_session_lifecycle() {
    let daemon = Daemon::start(600);
    let workdir = tempfile::tempdir().unwrap();
    let mut alice = daemon.connect();
    alice.hello("alice");
    assert!(alice.list("alice").is_empty());

    let summary = alice.create("alice", "sess-1", Some(workdir.path()));
    assert_eq!(summary["session_id"], "sess-1");
    assert_eq!(summary["cols"], 100);
    assert_eq!(summary["rows"], 40);
    assert_eq!(summary["alive"], true);
    assert_eq!(
        summary["cwd"].as_str().unwrap(),
        workdir.path().to_string_lossy()
    );
    let pid = summary["pid"].as_u64().unwrap() as u32;
    assert!(Path::new(&format!("/proc/{pid}")).exists());

    // Run something, wait for the scrollback to fill, then attach: replay must
    // contain the output even though we were not attached when it happened.
    alice.input("sess-1", "printf 'REPLAY_MARK_OK\\n'\n");
    wait_until(
        || {
            alice
                .list("alice")
                .first()
                .map(|entry| entry["lines"].as_u64().unwrap_or(0) > 0)
                .unwrap_or(false)
        },
        WAIT,
        "session scrollback to fill",
    );

    let (ok, detail) = alice.attach("alice", "sess-1");
    assert!(ok, "attach failed: {detail:?}");
    assert!(
        alice.text().contains("REPLAY_MARK_OK"),
        "replay missing: {:?}",
        alice.text()
    );

    // Live output after attaching.
    alice.input("sess-1", "printf 'LIVE_MARK_OK\\n'\n");
    alice.read_data_until("LIVE_MARK_OK");

    let (ok, detail) = alice.resize("alice", "sess-1", 132, 43);
    assert!(ok);
    assert_eq!(detail.as_deref(), Some("132x43"));
    let listed = alice.list("alice");
    assert_eq!(listed[0]["cols"], 132);
    assert_eq!(listed[0]["rows"], 43);

    // A different namespace cannot see or attach to Alice's session.
    let mut bob = daemon.connect();
    bob.hello("bob");
    assert!(bob.list("bob").is_empty());
    let (ok, detail) = bob.attach("bob", "sess-1");
    assert!(!ok, "cross-namespace attach must fail");
    assert!(
        detail
            .unwrap_or_default()
            .contains("another client namespace"),
        "unexpected detail for cross-namespace attach"
    );

    // Detach leaves the process alive; destroy kills it.
    alice.send_value(
        &json!({"kind":"detach","client_id":"alice","session_id":"sess-1"}),
        b"",
    );
    alice.reply_ok();
    assert!(Path::new(&format!("/proc/{pid}")).exists());
    assert_eq!(alice.list("alice")[0]["alive"], true);

    alice.send_value(
        &json!({"kind":"destroy","client_id":"alice","session_id":"sess-1"}),
        b"",
    );
    alice.reply_ok();
    wait_until(
        || !Path::new(&format!("/proc/{pid}")).exists(),
        WAIT,
        "session process to disappear",
    );
    assert!(alice.list("alice").is_empty());
}

#[test]
fn second_serve_refuses_to_start() {
    let daemon = Daemon::start(600);
    let mut second = daemon.command();
    second
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = second.spawn().unwrap();
    let deadline = Instant::now() + WAIT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "second serve did not exit (first daemon log:\n{})",
            daemon.log()
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(4), "second serve must exit 4");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("already running"),
        "unexpected stderr: {stderr}"
    );
    // The original daemon is unaffected.
    let mut client = daemon.connect();
    client.hello("alice");
    assert!(client.list("alice").is_empty());
}

#[test]
fn idle_daemon_exits_and_removes_its_socket() {
    let mut daemon = Daemon::start(1);
    let status = daemon.wait_for_exit(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0));
    assert!(
        !daemon.socket.exists(),
        "socket should be removed on idle exit"
    );
}

#[test]
fn idle_timer_is_suspended_while_a_client_is_connected() {
    let mut daemon = Daemon::start(2);
    let mut client = daemon.connect();
    client.hello("alice");
    // Hold the connection past the idle timeout: the daemon must stay up.
    thread::sleep(Duration::from_secs(4));
    assert!(
        daemon.child.try_wait().unwrap().is_none(),
        "daemon exited although a client was connected:\n{}",
        daemon.log()
    );
    assert!(client.list("alice").is_empty());

    drop(client);
    let status = daemon.wait_for_exit(Duration::from_secs(20));
    assert_eq!(status.code(), Some(0));
}

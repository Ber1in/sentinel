//! `serve`: the long-running daemon.
//!
//! One process per user, guarded by an `flock` on `helper.lock`, listening on a
//! `0600` Unix socket inside a `0700` directory. Every connection gets a thread
//! and speaks the frame protocol from [`crate::frame`].
//!
//! Requests carry their `client_id`; the first one a connection sends binds that
//! connection to a namespace and later requests may not switch. `list`, `attach`
//! and every other lookup stay inside that namespace.
//!
//! Protocol note for client implementations: an `attach` writes the replayed
//! `data` frames *before* its `reply`. A client must therefore keep reading
//! frames while a request is outstanding instead of assuming the reply comes
//! first.
//!
//! Idle shutdown: when there is no session and no connected client for
//! `idle_timeout` (default 30 minutes, `SENTINEL_HELPER_IDLE_SECS` override) the
//! daemon exits and removes its socket.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::frame::Message;
use crate::paths;
use crate::registry::{Handle, Registry, DEFAULT_MAX_SESSIONS};
use crate::session::{Session, SessionConfig};
use crate::wire::{self, ConnectionOut};

/// Default idle timeout (design decision D6).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Set by the SIGTERM/SIGINT handler, polled by the accept loop.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Everything `serve` needs to start.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Socket to listen on.
    pub socket: PathBuf,
    /// Single-instance lock file.
    pub lock: PathBuf,
    /// Exit after this long with no sessions and no clients.
    pub idle_timeout: Duration,
    /// Maximum concurrent sessions (`0` = unlimited).
    pub max_sessions: usize,
    /// Per-session settings.
    pub sessions: SessionConfig,
}

impl ServerConfig {
    /// Build a config from the environment (the paths come from [`crate::paths`]).
    pub fn from_env() -> Self {
        Self {
            socket: paths::socket_path(),
            lock: paths::lock_path(),
            idle_timeout: Duration::from_secs(
                env_u64("SENTINEL_HELPER_IDLE_SECS")
                    .unwrap_or(DEFAULT_IDLE_TIMEOUT.as_secs()),
            ),
            max_sessions: env_u64("SENTINEL_HELPER_MAX_SESSIONS")
                .map(|value| value as usize)
                .unwrap_or(DEFAULT_MAX_SESSIONS),
            sessions: SessionConfig::from_env(),
        }
    }

    /// Deterministic settings for in-process tests.
    #[cfg(test)]
    pub fn for_tests(socket: PathBuf) -> Self {
        let lock = socket
            .parent()
            .map(|dir| dir.join("helper.lock"))
            .unwrap_or_else(|| PathBuf::from("helper.lock"));
        Self {
            socket,
            lock,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            max_sessions: DEFAULT_MAX_SESSIONS,
            sessions: SessionConfig::for_tests(),
        }
    }
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse().ok()
}

/// Why `serve` refused to start.
#[derive(Debug, Error)]
pub enum ServeError {
    /// The single-instance lock is held by another process.
    #[error("another sentinel-helper is already running (lock: {0})")]
    AlreadyRunning(PathBuf),
    /// Anything else.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<io::Error> for ServeError {
    fn from(err: io::Error) -> Self {
        Self::Other(err.into())
    }
}

/// Run the daemon with settings from the environment.
pub fn run() -> Result<(), ServeError> {
    run_with(ServerConfig::from_env())
}

/// Run the daemon until it is signalled or goes idle.
pub fn run_with(config: ServerConfig) -> Result<(), ServeError> {
    let _instance = SingleInstance::acquire(&config.lock)?;
    install_signal_handlers();
    SHUTDOWN.store(false, Ordering::SeqCst);

    if let Some(parent) = config.socket.parent() {
        paths::ensure_private_dir(parent)?;
    }
    // A stale socket from a crashed daemon is expected; we hold the lock, so no
    // live instance can own this path.
    let _ = fs::remove_file(&config.socket);
    let listener = UnixListener::bind(&config.socket).map_err(|err| {
        ServeError::Other(anyhow::anyhow!(
            "cannot bind {}: {err}",
            config.socket.display()
        ))
    })?;
    paths::restrict_file(&config.socket)?;
    listener.set_nonblocking(true)?;

    let max_sessions = config.max_sessions;
    let context = Arc::new(Context {
        config: Arc::new(config),
        registry: Arc::new(Registry::new(max_sessions)),
        connections: AtomicUsize::new(0),
        last_activity: Mutex::new(Instant::now()),
    });

    eprintln!(
        "sentinel-helper {} listening on {} (idle {}s)",
        env!("CARGO_PKG_VERSION"),
        context.config.socket.display(),
        context.config.idle_timeout.as_secs()
    );

    loop {
        loop {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    // Accepted sockets do not inherit non-blocking mode on Linux,
                    // but be explicit so frame I/O is always blocking.
                    if let Err(err) = stream.set_nonblocking(false) {
                        eprintln!("cannot configure connection: {err}");
                        continue;
                    }
                    context.touch();
                    let context = Arc::clone(&context);
                    thread::spawn(move || context.serve_connection(stream));
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    eprintln!("accept failed: {err}");
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }

        context.registry.prune();

        if SHUTDOWN.load(Ordering::SeqCst) {
            eprintln!("shutdown signal received");
            break;
        }
        if context.is_idle() {
            eprintln!("idle timeout reached, exiting");
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    drop(listener);
    for session in context.registry.take_all() {
        let _ = session.destroy();
    }
    let _ = fs::remove_file(&context.config.socket);
    Ok(())
}

fn install_signal_handlers() {
    extern "C" fn on_signal(_signal: libc::c_int) {
        SHUTDOWN.store(true, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
}

/// Holds the `flock` for as long as the daemon runs.
struct SingleInstance {
    file: File,
}

impl SingleInstance {
    fn acquire(path: &Path) -> Result<Self, ServeError> {
        if let Some(parent) = path.parent() {
            paths::ensure_private_dir(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                return Err(ServeError::AlreadyRunning(path.to_path_buf()));
            }
            return Err(err.into());
        }
        // Diagnostic only: the lock itself is the authority.
        let mut handle = &file;
        let _ = handle.set_len(0);
        let _ = writeln!(handle, "{}", std::process::id());
        Ok(Self { file })
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

struct Context {
    config: Arc<ServerConfig>,
    registry: Arc<Registry<Session>>,
    connections: AtomicUsize,
    last_activity: Mutex<Instant>,
}

impl Context {
    fn touch(&self) {
        *lock(&self.last_activity) = Instant::now();
    }

    fn elapsed_idle(&self) -> Duration {
        lock(&self.last_activity).elapsed()
    }

    fn is_idle(&self) -> bool {
        self.registry.is_empty()
            && self.connections.load(Ordering::SeqCst) == 0
            && self.elapsed_idle() >= self.config.idle_timeout
    }

    fn serve_connection(self: &Arc<Self>, stream: UnixStream) {
        self.connections.fetch_add(1, Ordering::SeqCst);
        let result = self.handle_connection(stream);
        self.connections.fetch_sub(1, Ordering::SeqCst);
        self.touch();
        if let Err(err) = result {
            eprintln!("connection closed with error: {err}");
        }
    }

    fn handle_connection(&self, stream: UnixStream) -> io::Result<()> {
        let out = Arc::new(ConnectionOut::new(stream.try_clone()?));
        let mut handler = Handler::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.config),
            out,
        );
        let mut reader = BufReader::new(stream);
        loop {
            match wire::read_frame(&mut reader) {
                Ok(None) => break,
                Ok(Some((message, body))) => {
                    if !handler.handle(&message, &body)? {
                        break;
                    }
                }
                Err(err) => return Err(err),
            }
        }
        handler.shutdown();
        Ok(())
    }
}

/// Routes one connection's messages. Kept separate from the socket loop so it
/// can be exercised in-process by tests.
pub struct Handler {
    registry: Arc<Registry<Session>>,
    config: Arc<ServerConfig>,
    out: Arc<ConnectionOut>,
    namespace: Option<String>,
    attached: HashMap<String, Arc<Session>>,
    next_reply: u64,
}

impl Handler {
    /// Create a handler that writes replies and terminal data to `out`.
    pub fn new(
        registry: Arc<Registry<Session>>,
        config: Arc<ServerConfig>,
        out: Arc<ConnectionOut>,
    ) -> Self {
        Self {
            registry,
            config,
            out,
            namespace: None,
            attached: HashMap::new(),
            next_reply: 0,
        }
    }

    /// Handle one frame. Returns `false` when the connection should be closed.
    pub fn handle(&mut self, message: &Message, body: &[u8]) -> io::Result<bool> {
        match message {
            Message::Hello { client_id, version } => {
                if let Err(reason) = self.bind(client_id) {
                    self.reply_err(reason)?;
                    return Ok(true);
                }
                let detail = format!(
                    "helper {} (client protocol {})",
                    env!("CARGO_PKG_VERSION"),
                    version
                );
                self.reply(true, Some(detail))?;
            }
            Message::List { client_id } => {
                if let Err(reason) = self.bind(client_id) {
                    self.reply_err(reason)?;
                    return Ok(true);
                }
                let sessions = self.registry.list(client_id);
                let detail = serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".into());
                self.reply(true, Some(detail))?;
            }
            Message::Create {
                client_id,
                session_id,
                cwd,
                cols,
                rows,
            } => {
                if let Err(reason) = self.bind(client_id) {
                    self.reply_err(reason)?;
                    return Ok(true);
                }
                self.create(client_id, session_id, cwd.as_deref(), *cols, *rows)?;
            }
            Message::Attach {
                client_id,
                session_id,
            } => {
                if let Err(reason) = self.bind(client_id) {
                    self.reply_err(reason)?;
                    return Ok(true);
                }
                self.attach(client_id, session_id)?;
            }
            Message::Detach {
                client_id,
                session_id,
            } => {
                if let Err(reason) = self.bind(client_id) {
                    self.reply_err(reason)?;
                    return Ok(true);
                }
                if let Some(session) = self.attached.remove(session_id) {
                    session.detach(&self.out);
                } else if let Ok(session) = self.registry.get(client_id, session_id) {
                    session.detach(&self.out);
                }
                self.reply(true, None)?;
            }
            Message::Resize {
                client_id,
                session_id,
                cols,
                rows,
            } => {
                if let Err(reason) = self.bind(client_id) {
                    self.reply_err(reason)?;
                    return Ok(true);
                }
                match self.registry.get(client_id, session_id) {
                    Ok(session) => match session.resize(*cols, *rows) {
                        Ok(()) => self.reply(true, Some(format!("{}x{}", cols, rows)))?,
                        Err(err) => self.reply_err(format!("resize failed: {err}"))?,
                    },
                    Err(err) => self.reply_err(err.to_string())?,
                }
            }
            Message::Destroy {
                client_id,
                session_id,
            } => {
                if let Err(reason) = self.bind(client_id) {
                    self.reply_err(reason)?;
                    return Ok(true);
                }
                self.destroy(client_id, session_id)?;
            }
            Message::Data {
                session_id, len, ..
            } => {
                let Some(namespace) = self.namespace.clone() else {
                    self.reply_err("hello is required before sending input")?;
                    return Ok(true);
                };
                if body.len() != *len as usize {
                    self.reply_err(format!(
                        "data length mismatch: header says {len}, got {}",
                        body.len()
                    ))?;
                    return Ok(true);
                }
                match self.registry.get(&namespace, session_id) {
                    Ok(session) => {
                        if let Err(err) = session.write(body) {
                            self.reply_err(format!("write failed: {err}"))?;
                        }
                    }
                    Err(err) => self.reply_err(err.to_string())?,
                }
            }
            Message::Reply { .. } | Message::Event { .. } => {
                self.reply_err("unexpected message kind from client")?;
            }
        }
        Ok(true)
    }

    /// Drop subscriptions when the connection goes away.
    pub fn shutdown(&mut self) {
        for (_, session) in self.attached.drain() {
            session.detach(&self.out);
        }
    }

    fn bind(&mut self, client_id: &str) -> Result<(), String> {
        if client_id.is_empty() {
            return Err("client_id is required".to_string());
        }
        match &self.namespace {
            Some(bound) if bound != client_id => Err(format!(
                "connection is bound to namespace {bound:?}, not {client_id:?}"
            )),
            Some(_) => Ok(()),
            None => {
                self.namespace = Some(client_id.to_string());
                Ok(())
            }
        }
    }

    fn create(
        &mut self,
        client_id: &str,
        session_id: &str,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> io::Result<()> {
        if session_id.is_empty() {
            self.reply_err("session_id is required")?;
            return Ok(());
        }
        let cwd_path = cwd.filter(|dir| !dir.is_empty()).map(PathBuf::from);
        let session = match Session::spawn(
            client_id,
            session_id,
            cwd_path.as_deref(),
            cols,
            rows,
            &self.config.sessions,
        ) {
            Ok(session) => session,
            Err(err) => {
                self.reply_err(format!("cannot create session: {err}"))?;
                return Ok(());
            }
        };
        let summary = session.summary();
        if let Err(err) = self.registry.insert(Arc::clone(&session)) {
            let _ = session.destroy();
            self.reply_err(err.to_string())?;
            return Ok(());
        }
        let detail = serde_json::to_string(&summary).ok();
        self.reply(true, detail)
    }

    fn attach(&mut self, client_id: &str, session_id: &str) -> io::Result<()> {
        let session = match self.registry.get(client_id, session_id) {
            Ok(session) => session,
            Err(err) => {
                self.reply_err(err.to_string())?;
                return Ok(());
            }
        };
        if !session.is_alive() {
            self.reply_err("session has exited")?;
            return Ok(());
        }
        let replay = self.config.sessions.replay_lines;
        match session.attach(Arc::clone(&self.out), replay) {
            Ok(bytes) => {
                self.attached.insert(session_id.to_string(), session);
                self.reply(
                    true,
                    Some(format!("{{\"replayed_bytes\":{bytes}}}")),
                )?;
            }
            Err(err) => self.reply_err(format!("attach failed: {err}"))?,
        }
        Ok(())
    }

    fn destroy(&mut self, client_id: &str, session_id: &str) -> io::Result<()> {
        self.attached.remove(session_id);
        match self.registry.remove(client_id, session_id) {
            Ok(session) => match session.destroy() {
                Ok(()) => self.reply(true, None)?,
                Err(err) => self.reply_err(format!("destroy failed: {err}"))?,
            },
            Err(err) => self.reply_err(err.to_string())?,
        }
        Ok(())
    }

    fn reply(&mut self, ok: bool, detail: Option<String>) -> io::Result<()> {
        let id = self.next_reply;
        self.next_reply += 1;
        self.out.send_message(&Message::Reply { id, ok, detail })
    }

    fn reply_err(&mut self, detail: impl Into<String>) -> io::Result<()> {
        self.reply(false, Some(detail.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::SessionSummary;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    /// In-process client: sends frames and reads replies synchronously.
    struct Peer {
        socket: UnixStream,
    }

    impl Peer {
        fn send(&mut self, message: &Message, body: &[u8]) {
            wire::write_frame(&mut self.socket, message, body).unwrap();
        }

        /// Read frames until a `reply` arrives, collecting any `data` bodies.
        fn read_until_reply(&mut self, collected: &mut Vec<u8>) -> (bool, Option<String>) {
            loop {
                match wire::read_frame(&mut self.socket) {
                    Ok(Some((Message::Reply { ok, detail, .. }, _))) => return (ok, detail),
                    Ok(Some((Message::Data { .. }, body))) => collected.extend_from_slice(&body),
                    Ok(Some((Message::Event { .. }, _))) => {}
                    Ok(Some((other, _))) => panic!("unexpected frame: {other:?}"),
                    Ok(None) => panic!("connection closed while waiting for a reply"),
                    Err(err) => panic!("read failed: {err}"),
                }
            }
        }

        fn reply(&mut self) -> (bool, Option<String>) {
            self.read_until_reply(&mut Vec::new())
        }

        fn reply_ok(&mut self) -> Option<String> {
            let (ok, detail) = self.reply();
            assert!(ok, "expected ok reply, got failure: {detail:?}");
            detail
        }

        fn reply_err(&mut self) -> String {
            let (ok, detail) = self.reply();
            assert!(!ok, "expected failure reply, got ok: {detail:?}");
            detail.unwrap_or_default()
        }

        /// Read data frames until `needle` shows up.
        fn read_data_until(&mut self, needle: &str) {
            let mut collected = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match wire::read_frame(&mut self.socket) {
                    Ok(Some((Message::Data { .. }, body))) => {
                        collected.extend_from_slice(&body);
                        let text = String::from_utf8_lossy(&collected);
                        if text.contains(needle) {
                            return;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("connection closed while waiting for {needle:?}"),
                    Err(err) => panic!("read failed: {err}"),
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {needle:?}; got {:?}",
                    String::from_utf8_lossy(&collected)
                );
            }
        }
    }

    fn handler_pair(config: ServerConfig) -> (Handler, Peer, Arc<Registry<Session>>) {
        let registry = Arc::new(Registry::new(config.max_sessions));
        let (server, client) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let out = Arc::new(ConnectionOut::new(server));
        let handler = Handler::new(Arc::clone(&registry), Arc::new(config), out);
        (handler, Peer { socket: client }, registry)
    }

    fn test_config() -> ServerConfig {
        ServerConfig::for_tests(PathBuf::from("/tmp/unused-helper-test.sock"))
    }

    fn hello(peer: &mut Peer, client_id: &str) {
        peer.send(
            &Message::Hello {
                client_id: client_id.into(),
                version: "0.1.0".into(),
            },
            b"",
        );
        let detail = peer.reply_ok().unwrap_or_default();
        assert!(detail.contains("helper"), "unexpected hello detail: {detail}");
    }

    #[test]
    fn hello_reports_the_helper_version() {
        let (_handler, mut peer, _registry) = handler_pair(test_config());
        peer.send(
            &Message::Hello {
                client_id: "alice".into(),
                version: "0.1.0".into(),
            },
            b"",
        );
        let detail = peer.reply_ok().unwrap();
        assert!(detail.contains(env!("CARGO_PKG_VERSION")));
        assert!(detail.contains("0.1.0"));
    }

    #[test]
    fn empty_client_id_is_rejected() {
        let (_handler, mut peer, _registry) = handler_pair(test_config());
        peer.send(
            &Message::Hello {
                client_id: "".into(),
                version: "0.1.0".into(),
            },
            b"",
        );
        assert!(peer.reply_err().contains("client_id"));
    }

    #[test]
    fn a_connection_cannot_switch_namespaces() {
        let (_handler, mut peer, _registry) = handler_pair(test_config());
        peer.send(
            &Message::Hello {
                client_id: "alice".into(),
                version: "0.1.0".into(),
            },
            b"",
        );
        peer.reply_ok();
        peer.send(
            &Message::List {
                client_id: "bob".into(),
            },
            b"",
        );
        let detail = peer.reply_err();
        assert!(detail.contains("alice"), "unexpected detail: {detail}");
    }

    #[test]
    fn input_before_any_identity_is_rejected() {
        let (_handler, mut peer, _registry) = handler_pair(test_config());
        peer.send(
            &Message::Data {
                session_id: "s1".into(),
                seq: 1,
                len: 2,
            },
            b"hi",
        );
        assert!(peer.reply_err().contains("hello"));
    }

    #[test]
    fn create_attach_and_destroy_roundtrip() {
        let (mut _handler, mut peer, registry) = handler_pair(test_config());
        hello(&mut peer, "alice");

        peer.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "sess-1".into(),
                cwd: None,
                cols: 100,
                rows: 30,
            },
            b"",
        );
        let detail = peer.reply_ok().unwrap();
        let summary: SessionSummary = serde_json::from_str(&detail).unwrap();
        assert_eq!(summary.session_id, "sess-1");
        assert_eq!(summary.cols, 100);
        assert_eq!(summary.rows, 30);
        assert!(summary.alive);
        let pid = summary.pid;

        let command = b"echo HELLO\n";
        peer.send(
            &Message::Data {
                session_id: "sess-1".into(),
                seq: 1,
                len: command.len() as u32,
            },
            command,
        );
        // Wait for the shell to echo, then attach: the replay must contain it.
        let session = registry.get("alice", "sess-1").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&session.snapshot(usize::MAX)).contains("HELLO") {
            assert!(Instant::now() < deadline, "shell produced no output");
            thread::sleep(Duration::from_millis(20));
        }
        peer.send(
            &Message::Attach {
                client_id: "alice".into(),
                session_id: "sess-1".into(),
            },
            b"",
        );
        let mut collected = Vec::new();
        let (ok, detail) = peer.read_until_reply(&mut collected);
        assert!(ok, "attach failed: {detail:?}");
        assert!(
            String::from_utf8_lossy(&collected).contains("HELLO"),
            "replay before reply was empty: {:?}",
            String::from_utf8_lossy(&collected)
        );

        let command = b"echo LIVE\n";
        peer.send(
            &Message::Data {
                session_id: "sess-1".into(),
                seq: 2,
                len: command.len() as u32,
            },
            command,
        );
        peer.read_data_until("LIVE");

        peer.send(
            &Message::Resize {
                client_id: "alice".into(),
                session_id: "sess-1".into(),
                cols: 120,
                rows: 40,
            },
            b"",
        );
        assert_eq!(peer.reply_ok().as_deref(), Some("120x40"));
        assert_eq!(registry.list("alice")[0].cols, 120);

        peer.send(
            &Message::Destroy {
                client_id: "alice".into(),
                session_id: "sess-1".into(),
            },
            b"",
        );
        peer.reply_ok();
        assert!(registry.is_empty());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Path::new(&format!("/proc/{pid}")).exists() {
            assert!(Instant::now() < deadline, "session process survived destroy");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn list_only_returns_the_callers_sessions() {
        let (mut _handler, mut alice, registry) = handler_pair(test_config());
        hello(&mut alice, "alice");
        alice.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "alice-1".into(),
                cwd: None,
                cols: 80,
                rows: 24,
            },
            b"",
        );
        alice.reply_ok();

        alice.send(
            &Message::List {
                client_id: "alice".into(),
            },
            b"",
        );
        let detail = alice.reply_ok().unwrap();
        let summaries: Vec<SessionSummary> = serde_json::from_str(&detail).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, "alice-1");

        // Bob talks to the same registry on a separate connection.
        let (server, client) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut bob = Peer { socket: client };
        let mut bob_handler = Handler::new(
            Arc::clone(&registry),
            Arc::new(test_config()),
            Arc::new(ConnectionOut::new(server)),
        );
        hello(&mut bob, "bob");
        bob.send(
            &Message::List {
                client_id: "bob".into(),
            },
            b"",
        );
        let detail = bob.reply_ok().unwrap();
        assert_eq!(detail, "[]");

        // Bob may not attach to Alice's session.
        bob.send(
            &Message::Attach {
                client_id: "bob".into(),
                session_id: "alice-1".into(),
            },
            b"",
        );
        let detail = bob.reply_err();
        assert!(
            detail.contains("another client namespace"),
            "unexpected detail: {detail}"
        );

        // Alice still can.
        alice.send(
            &Message::Destroy {
                client_id: "alice".into(),
                session_id: "alice-1".into(),
            },
            b"",
        );
        alice.reply_ok();
    }

    #[test]
    fn detach_keeps_the_session_alive_and_reattach_works() {
        let (mut _handler, mut peer, registry) = handler_pair(test_config());
        hello(&mut peer, "alice");
        peer.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "s1".into(),
                cwd: None,
                cols: 80,
                rows: 24,
            },
            b"",
        );
        peer.reply_ok();

        peer.send(
            &Message::Attach {
                client_id: "alice".into(),
                session_id: "s1".into(),
            },
            b"",
        );
        peer.reply_ok();

        peer.send(
            &Message::Detach {
                client_id: "alice".into(),
                session_id: "s1".into(),
            },
            b"",
        );
        peer.reply_ok();
        let session = registry.get("alice", "s1").unwrap();
        assert!(session.is_alive());
        assert_eq!(session.subscriber_count(), 0);

        peer.send(
            &Message::Attach {
                client_id: "alice".into(),
                session_id: "s1".into(),
            },
            b"",
        );
        peer.reply_ok();
        assert_eq!(session.subscriber_count(), 1);

        peer.send(
            &Message::Destroy {
                client_id: "alice".into(),
                session_id: "s1".into(),
            },
            b"",
        );
        peer.reply_ok();
    }

    #[test]
    fn unknown_sessions_report_not_found() {
        let (mut _handler, mut peer, _registry) = handler_pair(test_config());
        hello(&mut peer, "alice");
        peer.send(
            &Message::Attach {
                client_id: "alice".into(),
                session_id: "missing".into(),
            },
            b"",
        );
        assert!(peer.reply_err().contains("not found"));
        peer.send(
            &Message::Destroy {
                client_id: "alice".into(),
                session_id: "missing".into(),
            },
            b"",
        );
        assert!(peer.reply_err().contains("not found"));
    }

    #[test]
    fn session_limit_is_reported() {
        let mut config = test_config();
        config.max_sessions = 1;
        let (mut _handler, mut peer, _registry) = handler_pair(config);
        hello(&mut peer, "alice");
        peer.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "s1".into(),
                cwd: None,
                cols: 80,
                rows: 24,
            },
            b"",
        );
        peer.reply_ok();

        peer.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "s2".into(),
                cwd: None,
                cols: 80,
                rows: 24,
            },
            b"",
        );
        assert!(peer.reply_err().contains("limit reached"));

        peer.send(
            &Message::Destroy {
                client_id: "alice".into(),
                session_id: "s1".into(),
            },
            b"",
        );
        peer.reply_ok();
    }

    #[test]
    fn create_with_missing_cwd_fails_cleanly() {
        let (mut _handler, mut peer, registry) = handler_pair(test_config());
        hello(&mut peer, "alice");
        peer.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "s1".into(),
                cwd: Some("/no/such/directory".into()),
                cols: 80,
                rows: 24,
            },
            b"",
        );
        let detail = peer.reply_err();
        assert!(detail.contains("cannot create session"), "detail: {detail}");
        assert!(registry.is_empty());
    }

    #[test]
    fn data_length_mismatch_is_rejected() {
        let (mut _handler, mut peer, _registry) = handler_pair(test_config());
        hello(&mut peer, "alice");
        peer.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "s1".into(),
                cwd: None,
                cols: 80,
                rows: 24,
            },
            b"",
        );
        peer.reply_ok();
        peer.send(
            &Message::Data {
                session_id: "s1".into(),
                seq: 1,
                len: 100,
            },
            b"short",
        );
        assert!(peer.reply_err().contains("length mismatch"));
        peer.send(
            &Message::Destroy {
                client_id: "alice".into(),
                session_id: "s1".into(),
            },
            b"",
        );
        peer.reply_ok();
    }

    #[test]
    fn shutdown_detaches_subscriptions() {
        let (mut handler, mut peer, registry) = handler_pair(test_config());
        hello(&mut peer, "alice");
        peer.send(
            &Message::Create {
                client_id: "alice".into(),
                session_id: "s1".into(),
                cwd: None,
                cols: 80,
                rows: 24,
            },
            b"",
        );
        peer.reply_ok();
        peer.send(
            &Message::Attach {
                client_id: "alice".into(),
                session_id: "s1".into(),
            },
            b"",
        );
        peer.reply_ok();
        let session = registry.get("alice", "s1").unwrap();
        assert_eq!(session.subscriber_count(), 1);
        handler.shutdown();
        assert_eq!(session.subscriber_count(), 0);
        session.destroy().unwrap();
    }

    #[test]
    fn idle_requires_no_sessions_and_no_clients() {
        let config = ServerConfig {
            idle_timeout: Duration::from_millis(0),
            ..test_config()
        };
        let context = Context {
            config: Arc::new(config),
            registry: Arc::new(Registry::new(4)),
            connections: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now() - Duration::from_secs(1)),
        };
        assert!(context.is_idle());
        context.connections.fetch_add(1, Ordering::SeqCst);
        assert!(!context.is_idle());
        context.connections.fetch_sub(1, Ordering::SeqCst);
        context.touch();
        assert!(!context.is_idle());
    }
}

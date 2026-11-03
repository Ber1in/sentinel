//! PTY-backed terminal sessions.
//!
//! A session owns one `openpty` pair plus the child process running on it. The
//! child is started with `setsid()`, which makes it the leader of a new process
//! group *and* a new session, so [`Session::destroy`] can take down the whole
//! job with `killpg`.
//!
//! Output is written to a [`RingBuffer`] and fanned out to attached clients.
//! Detaching only drops the subscriber; the process keeps running.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::frame::Message;
use crate::registry::{now_secs, Handle, SessionSummary};
use crate::ring::{RingBuffer, DEFAULT_BYTES, DEFAULT_LINES};
use crate::wire::ConnectionOut;

/// Largest terminal payload put into one `data` frame.
pub const MAX_DATA_BODY: usize = 32 * 1024;

/// Default number of lines replayed when a client attaches.
pub const DEFAULT_REPLAY_LINES: usize = 2_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Settings shared by every session a daemon creates.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Shell to run; defaults to `$SHELL` or `/bin/sh`.
    pub shell: PathBuf,
    /// Pass `-l` so the shell starts as a login shell.
    pub login_shell: bool,
    /// Scrollback capacity in lines (`0` disables the buffer).
    pub ring_lines: usize,
    /// Scrollback byte budget.
    pub ring_bytes: usize,
    /// Lines replayed on attach.
    pub replay_lines: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            shell: default_shell(),
            login_shell: true,
            ring_lines: DEFAULT_LINES,
            ring_bytes: DEFAULT_BYTES,
            replay_lines: DEFAULT_REPLAY_LINES,
        }
    }
}

impl SessionConfig {
    /// Read the optional environment overrides used by the daemon.
    ///
    /// - `SHELL` – shell to run (default `/bin/sh`).
    /// - `SENTINEL_HELPER_LOGIN_SHELL` – `0`/`false` runs the shell without `-l`.
    /// - `SENTINEL_HELPER_RING_LINES` – scrollback lines (default 100k, `0` disables).
    /// - `SENTINEL_HELPER_RING_BYTES` – scrollback byte budget (default 16 MiB).
    /// - `SENTINEL_HELPER_REPLAY_LINES` – lines replayed on attach (default 2000).
    pub fn from_env() -> Self {
        let default = Self::default();
        Self {
            shell: default.shell,
            login_shell: env_bool("SENTINEL_HELPER_LOGIN_SHELL").unwrap_or(default.login_shell),
            ring_lines: env_usize("SENTINEL_HELPER_RING_LINES").unwrap_or(default.ring_lines),
            ring_bytes: env_usize("SENTINEL_HELPER_RING_BYTES").unwrap_or(default.ring_bytes),
            replay_lines: env_usize("SENTINEL_HELPER_REPLAY_LINES").unwrap_or(default.replay_lines),
        }
    }

    /// Deterministic settings for tests: no login shell, small buffers.
    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self {
            shell: PathBuf::from("/bin/sh"),
            login_shell: false,
            ring_lines: 4_096,
            ring_bytes: 8 * 1024 * 1024,
            replay_lines: 512,
        }
    }
}

fn default_shell() -> PathBuf {
    match std::env::var_os("SHELL") {
        Some(shell) if !shell.is_empty() => PathBuf::from(shell),
        _ => PathBuf::from("/bin/sh"),
    }
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.trim().parse().ok()
}

fn env_bool(key: &str) -> Option<bool> {
    let value = std::env::var(key).ok()?;
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn terminal_type() -> String {
    match std::env::var("TERM") {
        Ok(term) if !term.is_empty() => term,
        _ => "xterm-256color".to_string(),
    }
}

/// One PTY session.
pub struct Session {
    client_id: String,
    session_id: String,
    pid: u32,
    master: File,
    child: Mutex<Child>,
    ring: Mutex<RingBuffer>,
    subscribers: Mutex<Vec<Arc<ConnectionOut>>>,
    size: Mutex<(u16, u16)>,
    cwd: Option<String>,
    created_at: u64,
    seq: AtomicU64,
    alive: AtomicBool,
    exited: AtomicBool,
}

impl Session {
    /// Allocate a PTY and start `config.shell` on it.
    ///
    /// `cwd` is the initial working directory (the daemon's cwd when `None`).
    /// Rows/columns are applied before the shell runs so full-screen programs
    /// see the real size from their first output.
    pub fn spawn(
        client_id: &str,
        session_id: &str,
        cwd: Option<&Path>,
        cols: u16,
        rows: u16,
        config: &SessionConfig,
    ) -> io::Result<Arc<Session>> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let win = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let (master, slave) = open_pty()?;
        set_cloexec(&master)?;
        set_cloexec(&slave)?;

        let mut command = Command::new(&config.shell);
        if config.login_shell {
            command.arg("-l");
        }
        command
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));
        if let Some(dir) = cwd {
            if !dir.as_os_str().is_empty() {
                command.current_dir(dir);
            }
        }
        command.env("TERM", terminal_type());
        // Documented-safe closure: only async-signal-safe libc calls happen
        // between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSWINSZ, &win) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = command.spawn()?;
        // Drop the parent's slave copies so reading the master sees EOF/EIO when
        // the shell exits.
        drop(command);
        drop(slave);

        let pid = child.id();
        let session = Arc::new(Session {
            client_id: client_id.to_string(),
            session_id: session_id.to_string(),
            pid,
            master,
            child: Mutex::new(child),
            ring: Mutex::new(RingBuffer::new(config.ring_lines, config.ring_bytes)),
            subscribers: Mutex::new(Vec::new()),
            size: Mutex::new((cols, rows)),
            cwd: cwd.map(|dir| dir.to_string_lossy().into_owned()),
            created_at: now_secs(),
            seq: AtomicU64::new(0),
            alive: AtomicBool::new(true),
            exited: AtomicBool::new(false),
        });

        let reader = Arc::clone(&session);
        let master_reader = session.master.try_clone()?;
        if let Err(err) = std::thread::Builder::new()
            .name(format!("session-{session_id}"))
            .spawn(move || reader.read_loop(master_reader))
        {
            let _ = session.destroy();
            return Err(err);
        }
        Ok(session)
    }

    /// Client namespace that owns this session.
    #[allow(dead_code)] // accessor kept for symmetry with the registry
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Session identifier (unique per client).
    #[allow(dead_code)] // accessor kept for symmetry with the registry
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// PID of the shell, which is also the process-group id.
    #[allow(dead_code)] // reported by `list` and asserted by tests
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the shell is still running.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Copy of the current terminal size as `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        *lock(&self.size)
    }

    /// Attached client count (mostly useful for tests).
    #[allow(dead_code)]
    pub fn subscriber_count(&self) -> usize {
        lock(&self.subscribers).len()
    }

    /// Feed terminal input to the child.
    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let mut master = &self.master;
        master.write_all(data)?;
        master.flush()
    }

    /// Resize the PTY (and remember the value for `list`).
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let win = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        ioctl_winsize(self.master.as_raw_fd(), &win)?;
        *lock(&self.size) = (cols, rows);
        Ok(())
    }

    /// Snapshot of the last `lines` buffered lines (plus the in-flight line).
    #[allow(dead_code)] // used by tests and by future snapshot support
    pub fn snapshot(&self, lines: usize) -> Vec<u8> {
        lock(&self.ring).replay(lines)
    }

    /// Number of complete lines retained in the ring buffer.
    pub fn buffered_lines(&self) -> u64 {
        lock(&self.ring).line_count()
    }

    /// Attach a client: register it and replay buffered output.
    ///
    /// Returns the number of replayed bytes. Replay frames are written *before*
    /// the caller's own reply frame; clients must read frames continuously.
    pub fn attach(&self, out: Arc<ConnectionOut>, replay_lines: usize) -> io::Result<usize> {
        // The ring lock keeps replay and live output ordered: the reader thread
        // cannot append while the snapshot is on the wire.
        let ring = lock(&self.ring);
        let snapshot = ring.replay(replay_lines);
        {
            let mut subscribers = lock(&self.subscribers);
            if !subscribers
                .iter()
                .any(|existing| Arc::ptr_eq(existing, &out))
            {
                subscribers.push(Arc::clone(&out));
            }
        }
        for chunk in snapshot.chunks(MAX_DATA_BODY) {
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let message = Message::Data {
                session_id: self.session_id.clone(),
                seq,
                len: chunk.len() as u32,
            };
            out.send(&message, chunk)?;
        }
        drop(ring);
        Ok(snapshot.len())
    }

    /// Stop pushing output to a client, leaving the process running.
    pub fn detach(&self, out: &Arc<ConnectionOut>) {
        lock(&self.subscribers).retain(|existing| !Arc::ptr_eq(existing, out));
    }

    /// Kill the whole process group and free the scrollback.
    ///
    /// `SIGTERM` first, then `SIGKILL` when the group does not go away within
    /// half a second.
    pub fn destroy(&self) -> io::Result<()> {
        let pid = self.pid as libc::pid_t;
        {
            let mut child = lock(&self.child);
            let already_exited = self.exited.load(Ordering::SeqCst);
            if !already_exited && child.try_wait()?.is_none() {
                // `pid` is the session/process-group leader (`setsid` in pre_exec).
                // Job-control shells put jobs in their own process groups, so also
                // sweep every process that still belongs to this session. Both are
                // safe while the leader has not been reaped: its pid cannot be
                // recycled before that.
                signal_session(pid, libc::SIGTERM);
                if !wait_timeout(&mut child, Duration::from_millis(500))? {
                    signal_session(pid, libc::SIGKILL);
                    let _ = wait_timeout(&mut child, Duration::from_secs(2))?;
                }
            }
        }

        self.exited.store(true, Ordering::SeqCst);
        self.alive.store(false, Ordering::SeqCst);
        lock(&self.ring).clear();

        let subscribers = std::mem::take(&mut *lock(&self.subscribers));
        let event = Message::Event {
            session_id: self.session_id.clone(),
            what: "destroyed".to_string(),
            detail: None,
        };
        for subscriber in subscribers {
            let _ = subscriber.send_message(&event);
        }
        Ok(())
    }

    fn read_loop(&self, mut master: File) {
        let mut buffer = vec![0u8; 8192];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => self.publish(&buffer[..read]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                // EIO is the normal "last slave fd closed" signal on Linux.
                Err(_) => break,
            }
        }
        self.finish();
    }

    fn publish(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let mut ring = lock(&self.ring);
        ring.push(data);

        let mut subscribers = lock(&self.subscribers);
        if subscribers.is_empty() {
            return;
        }
        let mut dead = Vec::new();
        for chunk in data.chunks(MAX_DATA_BODY) {
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let message = Message::Data {
                session_id: self.session_id.clone(),
                seq,
                len: chunk.len() as u32,
            };
            for (index, subscriber) in subscribers.iter().enumerate() {
                if subscriber.send(&message, chunk).is_err() && !dead.contains(&index) {
                    dead.push(index);
                }
            }
        }
        for index in dead.into_iter().rev() {
            subscribers.remove(index);
        }
    }

    fn finish(&self) {
        if self.exited.swap(true, Ordering::SeqCst) {
            return;
        }
        self.alive.store(false, Ordering::SeqCst);

        let code = {
            let mut child = lock(&self.child);
            child
                .try_wait()
                .ok()
                .flatten()
                .and_then(|status| status.code())
        };

        let subscribers = std::mem::take(&mut *lock(&self.subscribers));
        let event = Message::Event {
            session_id: self.session_id.clone(),
            what: "exit".to_string(),
            detail: code.map(|code| code.to_string()),
        };
        for subscriber in subscribers {
            let _ = subscriber.send_message(&event);
        }
    }
}

impl Handle for Session {
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn summary(&self) -> SessionSummary {
        let (cols, rows) = self.size();
        SessionSummary {
            session_id: self.session_id.clone(),
            pid: self.pid,
            cols,
            rows,
            alive: self.is_alive(),
            lines: self.buffered_lines(),
            cwd: self.cwd.clone(),
            created_at: self.created_at,
        }
    }
}

/// Signal the session leader's process group and every process still in the
/// session (job-control shells give each job its own group).
fn signal_session(session_id: libc::pid_t, signal: libc::c_int) {
    unsafe {
        libc::killpg(session_id, signal);
    }
    for pid in session_members(session_id) {
        if pid == std::process::id() as libc::pid_t {
            continue;
        }
        unsafe {
            libc::kill(pid, signal);
        }
    }
}

/// PIDs whose kernel session id is `session_id`, excluding the leader itself.
///
/// Reads `/proc/<pid>/stat`; field 6 is the session id. The comm field is
/// parenthesised and may contain spaces, so parsing starts after the last `)`.
fn session_members(session_id: libc::pid_t) -> Vec<libc::pid_t> {
    let mut members = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return members;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if pid == session_id {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some(rest) = stat.rsplit(')').next() else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let _state = fields.next();
        let _ppid = fields.next();
        let _pgrp = fields.next();
        if fields.next().and_then(|value| value.parse::<i32>().ok()) == Some(session_id) {
            members.push(pid);
        }
    }
    members
}

fn wait_timeout(child: &mut Child, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn open_pty() -> io::Result<(File, File)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // Ownership moves to `File`, which closes both on drop.
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    Ok((master, slave))
}

fn set_cloexec(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ioctl_winsize(fd: libc::c_int, win: &libc::winsize) -> io::Result<()> {
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, win) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::read_frame;
    use std::os::unix::net::UnixStream;

    type FrameLog = Arc<Mutex<Vec<(Message, Vec<u8>)>>>;

    /// Collects frames read from a socket on a background thread so tests can
    /// poll for text without putting timeouts on a partially-consumed frame.
    struct FrameSink {
        frames: FrameLog,
    }

    impl FrameSink {
        fn spawn(mut socket: UnixStream) -> Self {
            let frames = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&frames);
            std::thread::spawn(move || {
                while let Ok(Some(frame)) = read_frame(&mut socket) {
                    sink.lock().unwrap().push(frame);
                }
            });
            Self { frames }
        }

        fn text(&self) -> String {
            let frames = self.frames.lock().unwrap();
            let bytes: Vec<u8> = frames
                .iter()
                .flat_map(|(_, body)| body.iter().copied())
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }

        fn events(&self) -> Vec<String> {
            let frames = self.frames.lock().unwrap();
            frames
                .iter()
                .filter_map(|(message, _)| match message {
                    Message::Event { what, .. } => Some(what.clone()),
                    _ => None,
                })
                .collect()
        }

        fn data_seqs(&self) -> Vec<u64> {
            let frames = self.frames.lock().unwrap();
            frames
                .iter()
                .filter_map(|(message, _)| match message {
                    Message::Data { seq, .. } => Some(*seq),
                    _ => None,
                })
                .collect()
        }
    }

    fn session(config: &SessionConfig) -> Arc<Session> {
        Session::spawn("client-a", "sess-1", None, 80, 24, config).expect("spawn session")
    }

    fn wait_for_text(session: &Session, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let text = String::from_utf8_lossy(&session.snapshot(usize::MAX)).into_owned();
            if text.contains(needle) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; buffer was: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_sink_text(sink: &FrameSink, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let text = sink.text();
            if text.contains(needle) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; frames were: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn runs_a_command_in_the_pty() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        session.write(b"printf 'SENTINEL_READY\\n'\n").unwrap();
        let text = wait_for_text(&session, "SENTINEL_READY", Duration::from_secs(10));
        assert!(text.contains("SENTINEL_READY"));
        session.destroy().unwrap();
    }

    #[test]
    fn initial_size_is_applied_and_resize_reaches_the_shell() {
        let config = SessionConfig::for_tests();
        let session = Session::spawn("c", "s", None, 100, 30, &config).unwrap();
        assert_eq!(session.size(), (100, 30));

        session.write(b"stty size\n").unwrap();
        wait_for_text(&session, "30 100", Duration::from_secs(10));

        session.resize(132, 43).unwrap();
        assert_eq!(session.size(), (132, 43));
        session.write(b"stty size\n").unwrap();
        wait_for_text(&session, "43 132", Duration::from_secs(10));
        session.destroy().unwrap();
    }

    #[test]
    fn spawn_honours_the_initial_cwd() {
        let config = SessionConfig::for_tests();
        let dir = tempfile::tempdir().unwrap();
        let session = Session::spawn("c", "s", Some(dir.path()), 80, 24, &config).unwrap();
        session.write(b"pwd\n").unwrap();
        let text = wait_for_text(
            &session,
            dir.path().to_str().unwrap(),
            Duration::from_secs(10),
        );
        assert!(text.contains(dir.path().to_str().unwrap()));
        session.destroy().unwrap();
    }

    #[test]
    fn spawn_rejects_a_missing_cwd() {
        let config = SessionConfig::for_tests();
        let err = Session::spawn(
            "c",
            "s",
            Some(Path::new("/definitely/not/a/directory")),
            80,
            24,
            &config,
        )
        .err()
        .expect("spawning in a missing cwd must fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn detach_keeps_the_shell_alive() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        session.write(b"echo before-attach\n").unwrap();
        wait_for_text(&session, "before-attach", Duration::from_secs(10));

        let (server, client) = UnixStream::pair().unwrap();
        let sink = FrameSink::spawn(server);
        let out = Arc::new(ConnectionOut::new(client));
        session
            .attach(Arc::clone(&out), config.replay_lines)
            .unwrap();
        assert_eq!(session.subscriber_count(), 1);

        session.detach(&out);
        assert_eq!(session.subscriber_count(), 0);
        assert!(session.is_alive());

        session.write(b"echo after-detach\n").unwrap();
        wait_for_text(&session, "after-detach", Duration::from_secs(10));
        // The detached client must not receive the new output.
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !sink.text().contains("after-detach"),
            "detached client still received output: {:?}",
            sink.text()
        );
        assert!(session.is_alive());
        session.destroy().unwrap();
    }

    #[test]
    fn destroy_kills_the_process_group() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        session.write(b"printf 'PID=%s\\n' \"$$\"\n").unwrap();

        // The pty echoes the command too; only the printed line parses.
        let deadline = Instant::now() + Duration::from_secs(10);
        let pid: u32 = loop {
            let text = String::from_utf8_lossy(&session.snapshot(usize::MAX)).into_owned();
            if let Some(pid) = text.lines().find_map(|line| {
                // The prompt prefixes the printed line, so look for the marker.
                let value = line.split("PID=").nth(1)?;
                let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.is_empty() {
                    None
                } else {
                    digits.parse().ok()
                }
            }) {
                break pid;
            }
            assert!(Instant::now() < deadline, "no pid in output: {text:?}");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(pid, session.pid());
        assert!(Path::new(&format!("/proc/{pid}")).exists());

        session.destroy().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Path::new(&format!("/proc/{pid}")).exists() {
            assert!(Instant::now() < deadline, "process {pid} survived destroy");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!session.is_alive());
        assert_eq!(session.buffered_lines(), 0);
        // Destroying twice is harmless.
        session.destroy().unwrap();
    }

    #[test]
    fn attach_replays_buffered_output_then_follows_live_output() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        session.write(b"printf 'REPLAY-ME\\n'\n").unwrap();
        wait_for_text(&session, "REPLAY-ME", Duration::from_secs(10));

        let (server, client) = UnixStream::pair().unwrap();
        let sink = FrameSink::spawn(server);
        let out = Arc::new(ConnectionOut::new(client));
        let replayed = session.attach(out, config.replay_lines).unwrap();
        assert!(replayed > 0);
        wait_for_sink_text(&sink, "REPLAY-ME", Duration::from_secs(5));

        session.write(b"printf 'LIVE-AFTER-ATTACH\\n'\n").unwrap();
        wait_for_sink_text(&sink, "LIVE-AFTER-ATTACH", Duration::from_secs(10));

        // Replay came first, then live data; sequence numbers never go back.
        let seqs = sink.data_seqs();
        assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]));
        let text = sink.text();
        assert!(
            text.find("REPLAY-ME").unwrap() < text.find("LIVE-AFTER-ATTACH").unwrap(),
            "replay must precede live output: {text:?}"
        );
        session.destroy().unwrap();
    }

    #[test]
    fn exit_event_reaches_attached_clients() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        let (server, client) = UnixStream::pair().unwrap();
        let sink = FrameSink::spawn(server);
        session
            .attach(Arc::new(ConnectionOut::new(client)), 100)
            .unwrap();

        session.write(b"exit 7\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !sink.events().iter().any(|what| what == "exit") {
            assert!(Instant::now() < deadline, "no exit event");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!session.is_alive());
        session.destroy().unwrap();
    }

    #[test]
    fn destroy_kills_the_whole_process_group() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        // A background child stays in the shell's process group, so killpg must
        // take it down as well.
        session
            .write(b"sleep 300 & printf 'CHILD=%s\\n' \"$!\"\n")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let child_pid: u32 = loop {
            let text = String::from_utf8_lossy(&session.snapshot(usize::MAX)).into_owned();
            if let Some(pid) = text.lines().find_map(|line| {
                let value = line.split("CHILD=").nth(1)?;
                let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.is_empty() {
                    None
                } else {
                    digits.parse().ok()
                }
            }) {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "no background pid in output: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            Path::new(&format!("/proc/{child_pid}")).exists(),
            "background process should be running"
        );

        session.destroy().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Path::new(&format!("/proc/{child_pid}")).exists() {
            assert!(
                Instant::now() < deadline,
                "background process {child_pid} survived killpg"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn attached_client_that_disappears_is_dropped() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        let (server, client) = UnixStream::pair().unwrap();
        let out = Arc::new(ConnectionOut::new(client));
        session.attach(Arc::clone(&out), 10).unwrap();
        assert_eq!(session.subscriber_count(), 1);
        drop(server);

        // Generating output makes the failed write observable.
        session.write(b"echo gone\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while session.subscriber_count() != 0 {
            assert!(Instant::now() < deadline, "dead subscriber not removed");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(session.is_alive());
        session.destroy().unwrap();
    }

    #[test]
    fn empty_write_is_a_noop() {
        let config = SessionConfig::for_tests();
        let session = session(&config);
        session.write(b"").unwrap();
        session.destroy().unwrap();
    }
}

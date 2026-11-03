//! Measurement harness for `docs/perf.md` (tasks 3.5 / 10.2).
//!
//! Kept out of the product workspace on purpose: it is a developer tool, not shipped code,
//! and pulling it into `cargo test --workspace` would slow CI down for no product value.
//!
//! Subcommands:
//!
//!   replay              scrollback append + 20k-line replay latency (no sshd needed)
//!   idle-cpu            idle CPU/RSS of one live session (needs --features session + sshd)
//!
//! Every number is printed as `key=value` on stdout so `scripts/perf.sh` can parse it
//! without screen-scraping prose. Human-readable context goes to stderr.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use terminal::Scrollback;

/// Same value the daemon defaults to, so eviction behaviour matches production.
const CAPACITY_LINES: usize = 100_000;
/// The acceptance scenario in the proposal: 20k lines of output, then scroll all the way up.
const TOTAL_LINES: usize = 20_000;
/// How many lines the daemon replays to a GUI that attaches or reconnects.
const ATTACH_LINES: usize = 2_000;
/// PTY reads arrive in small batches, not one line at a time.
const CHUNK_BYTES: usize = 4 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("replay") => {
            replay();
            ExitCode::SUCCESS
        }
        Some("idle-cpu") => idle_cpu(parse_seconds(&args)),
        Some("list") => {
            println!("replay");
            println!("idle-cpu");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: sentinel-perf <replay|idle-cpu|list> [--seconds N]\n\
                 \n\
                 replay    scrollback append and replay latency over {TOTAL_LINES} lines\n\
                 idle-cpu  idle CPU of one live session; requires --features session and the\n\
                 \x20         sshd fixture (eval \"$(./scripts/test-sshd.sh start)\")"
            );
            ExitCode::from(2)
        }
    }
}

fn parse_seconds(args: &[String]) -> u64 {
    let mut seconds = 30;
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--seconds" {
            if let Some(value) = iter.next() {
                seconds = value.parse().unwrap_or(seconds);
            }
        }
    }
    seconds.clamp(3, 600)
}

// ---------------------------------------------------------------------------
// replay
// ---------------------------------------------------------------------------

fn replay_line(index: usize) -> String {
    format!(
        "2026-11-03T10:00:00.{:03}Z  INFO worker-{:02} handled request id={:08x} \
         path=/api/v1/projects/{}/items status=200 took={}ms\n",
        index % 1000,
        index % 16,
        (index as u64).wrapping_mul(2_654_435_761) % 0xffff_ffff,
        index % 97,
        index % 43
    )
}

/// Feed `lines` synthetic log lines the way a PTY does: ~4 KiB batches, split on newlines.
fn fill(scrollback: &mut Scrollback, lines: usize) -> (usize, usize) {
    let mut buffer = Vec::with_capacity(CHUNK_BYTES * 2);
    let mut chunks = 0;
    for index in 0..lines {
        buffer.extend_from_slice(replay_line(index).as_bytes());
        if buffer.len() >= CHUNK_BYTES {
            scrollback.append(&buffer);
            chunks += 1;
            buffer.clear();
        }
    }
    if !buffer.is_empty() {
        scrollback.append(&buffer);
        chunks += 1;
    }
    (chunks, scrollback.lines())
}

fn append_bench() -> (f64, f64, usize, usize) {
    let mut scrollback = Scrollback::new(CAPACITY_LINES);
    let started = Instant::now();
    let (chunks, lines) = fill(&mut scrollback, TOTAL_LINES);
    let elapsed = started.elapsed();
    let bytes = scrollback.tail_lines(TOTAL_LINES).len();
    (elapsed.as_secs_f64() * 1e3, bytes as f64, chunks, lines)
}

/// Scroll from the newest page back to the oldest retained line, exactly like the GUI's
/// "scroll up" paging does.
fn walk_to_oldest(scrollback: &Scrollback, page_lines: usize) -> Duration {
    let started = Instant::now();
    let mut cursor = scrollback.next_seq();
    loop {
        let (start, page) = scrollback.page_before(cursor, page_lines);
        if page.is_empty() || start >= cursor {
            break;
        }
        cursor = start;
    }
    started.elapsed()
}

fn percentiles(samples_ns: &[u128]) -> (f64, f64, f64) {
    let mut sorted = samples_ns.to_vec();
    sorted.sort_unstable();
    let at = |quantile: f64| -> f64 {
        let index = ((sorted.len() as f64 - 1.0) * quantile).round() as usize;
        sorted[index] as f64 / 1e6
    };
    (at(0.5), at(0.95), sorted[sorted.len() - 1] as f64 / 1e6)
}

fn replay() {
    let (append_ms, bytes, chunks, lines) = append_bench();
    let mib_per_second = (bytes / 1024.0 / 1024.0) / (append_ms / 1e3);
    eprintln!(
        "append: {lines} lines / {bytes} bytes in {chunks} chunks, {append_ms:.2} ms \
         ({mib_per_second:.1} MiB/s)"
    );
    println!("scrollback_lines={lines}");
    println!("scrollback_bytes={bytes}");
    println!("scrollback_chunks={chunks}");
    println!("scrollback_append_ms={append_ms:.3}");
    println!("scrollback_append_mib_s={mib_per_second:.1}");

    // Build one populated buffer and measure read paths against it: replay never mutates.
    let mut scrollback = Scrollback::new(CAPACITY_LINES);
    fill(&mut scrollback, TOTAL_LINES);

    let mut attach = Vec::with_capacity(200);
    for _ in 0..200 {
        let started = Instant::now();
        let replay = scrollback.tail_lines(ATTACH_LINES);
        attach.push(started.elapsed().as_nanos());
        debug_assert_eq!(scrollback.lines(), lines);
        debug_assert!(!replay.is_empty());
    }
    let (attach_median, attach_p95, attach_max) = percentiles(&attach);
    eprintln!(
        "attach replay ({ATTACH_LINES} lines): median {attach_median:.3} ms, p95 {attach_p95:.3} ms, \
         max {attach_max:.3} ms over 200 runs"
    );
    println!("replay_attach_lines={ATTACH_LINES}");
    println!("replay_attach_median_ms={attach_median:.3}");
    println!("replay_attach_p95_ms={attach_p95:.3}");

    let mut walk = Vec::with_capacity(50);
    for _ in 0..50 {
        walk.push(walk_to_oldest(&scrollback, ATTACH_LINES).as_nanos());
    }
    let (walk_median, walk_p95, walk_max) = percentiles(&walk);
    let pages = (TOTAL_LINES as f64 / ATTACH_LINES as f64).ceil();
    eprintln!(
        "scroll to oldest ({TOTAL_LINES} lines, {pages:.0} pages of {ATTACH_LINES}): \
         median {walk_median:.3} ms, p95 {walk_p95:.3} ms, max {walk_max:.3} ms over 50 runs"
    );
    println!("replay_walk_lines={TOTAL_LINES}");
    println!("replay_walk_median_ms={walk_median:.3}");
    println!("replay_walk_p95_ms={walk_p95:.3}");

    let mut resync = Vec::with_capacity(50);
    for _ in 0..50 {
        let started = Instant::now();
        let (_, bytes) = scrollback
            .since(0)
            .expect("nothing evicted at 100k capacity");
        resync.push(started.elapsed().as_nanos());
        debug_assert!(!bytes.is_empty(), "resync must return the whole log");
    }
    let (resync_median, resync_p95, _) = percentiles(&resync);
    eprintln!(
        "resync from seq 0 ({TOTAL_LINES} lines): median {resync_median:.3} ms, \
         p95 {resync_p95:.3} ms over 50 runs"
    );
    println!("replay_resync_median_ms={resync_median:.3}");
    println!("replay_resync_p95_ms={resync_p95:.3}");
}

// ---------------------------------------------------------------------------
// idle-cpu
// ---------------------------------------------------------------------------

#[cfg(not(feature = "session"))]
fn idle_cpu(_seconds: u64) -> ExitCode {
    eprintln!(
        "idle-cpu needs the `session` feature (daemon runtime + live SSH session):\n\
         \n    cargo run --release --features session --manifest-path scripts/perf/Cargo.toml -- idle-cpu\n\
         \nThe first build compiles the daemon and takes a few minutes."
    );
    ExitCode::from(3)
}

#[cfg(feature = "session")]
fn idle_cpu(seconds: u64) -> ExitCode {
    session::run(seconds)
}

#[cfg(feature = "session")]
mod session {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use daemon::fsm::ReconnectPolicy;
    use daemon::runner::{wait_for_state, Command, SessionRuntime, SessionSpec};
    use persistence::{PersistenceProvider, TmuxOptions, TmuxPersistence};
    use protocol::SessionState;
    use ssh::{ConnectOptions, Credentials, HostKeyPolicy, Keepalive};

    const SOCKET: &str = "sentinel-perf";

    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }

    /// Clock ticks per second for `/proc/<pid>/stat`; Linux exposes it via `getconf`.
    fn clock_ticks() -> f64 {
        let output = std::process::Command::new("getconf")
            .arg("CLK_TCK")
            .output()
            .expect("run getconf CLK_TCK");
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(100.0)
    }

    /// utime + stime of this process, in clock ticks.
    fn cpu_ticks() -> u64 {
        let stat = std::fs::read_to_string("/proc/self/stat").expect("read /proc/self/stat");
        // The comm field is parenthesised and may contain spaces, so split after the last ')'.
        let rest = stat
            .rsplit_once(')')
            .map(|(_, rest)| rest)
            .expect("comm field");
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let utime: u64 = fields[11].parse().expect("utime");
        let stime: u64 = fields[12].parse().expect("stime");
        utime + stime
    }

    /// Resident set size of this process in MiB.
    fn rss_mib() -> f64 {
        let statm = std::fs::read_to_string("/proc/self/statm").expect("read /proc/self/statm");
        let resident: u64 = statm
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
            .expect("resident pages");
        resident as f64 * 4096.0 / 1024.0 / 1024.0
    }

    /// CPU ticks this process burned while idling for `seconds` of wall time.
    ///
    /// Ticks are reported alongside the percentage because `/proc` only advances in
    /// 1/hz steps: at hz=100 and a 30 s window one tick is 0.033% of a core, so a
    /// rounded percentage would hide the real resolution.
    async fn sample_ticks(seconds: u64) -> u64 {
        let before = cpu_ticks();
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        cpu_ticks().saturating_sub(before)
    }

    fn ticks_to_percent(ticks: u64, hz: f64, seconds: u64) -> f64 {
        ticks as f64 / hz / seconds as f64 * 100.0
    }

    pub fn run(seconds: u64) -> ExitCode {
        let Some(host) = env("SSHTERM_TEST_HOST") else {
            eprintln!(
                "idle-cpu needs the sshd fixture. Run:\n\
                 \n    eval \"$(./scripts/test-sshd.sh start)\"   # needs root, once per boot\n"
            );
            return ExitCode::from(4);
        };
        let port: u16 = env("SSHTERM_TEST_PORT")
            .and_then(|value| value.parse().ok())
            .unwrap_or(2222);
        let user = env("SSHTERM_TEST_USER").unwrap_or_else(|| "sshtermtest".into());
        let password = env("SSHTERM_TEST_PASSWORD").unwrap_or_else(|| "sshtermtest-pw".into());

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        runtime.block_on(async move {
            let hz = clock_ticks();
            // Baseline: the harness itself does nothing but sleep, so this is the floor.
            let baseline_ticks = sample_ticks(3).await;
            let baseline = ticks_to_percent(baseline_ticks, hz, 3);
            eprintln!(
                "baseline (no session): {baseline_ticks} ticks = {baseline:.2}% of one core over 3 s"
            );

            let namespace = format!("perf{}", std::process::id());
            let provider = Arc::new(TmuxPersistence::new(TmuxOptions {
                socket: SOCKET.into(),
                config_path: format!("/tmp/sentinel-perf-{}/tmux.conf", std::process::id()),
                ..TmuxOptions::default()
            })) as Arc<dyn PersistenceProvider>;

            let spec = SessionSpec {
                session_id: "perf-idle".into(),
                namespace,
                host_id: "perf".into(),
                pane_id: "perf-1".into(),
                connect: ConnectOptions {
                    host,
                    port,
                    user,
                    credentials: Credentials::Password(password),
                    host_key_policy: HostKeyPolicy::AcceptAny,
                    keepalive: Keepalive::default(),
                    timeout: Duration::from_secs(10),
                },
                provider,
                cwd: Some("/tmp".into()),
                scrollback_lines: CAPACITY_LINES,
                spill_dir: None,
                spill_max_bytes: 0,
                policy: ReconnectPolicy::default(),
                term: "xterm-256color".into(),
                cols: 120,
                rows: 32,
            };

            let mut session = SessionRuntime::spawn(spec);
            if !wait_for_state(&mut session.events, SessionState::Connected).await {
                eprintln!("session never reached `connected`; check the fixture and tmux");
                return ExitCode::from(5);
            }
            eprintln!("session connected; sampling {seconds} s of idle");

            let idle_ticks = sample_ticks(seconds).await;
            let idle = ticks_to_percent(idle_ticks, hz, seconds);
            let state = *session.events.state.borrow();
            let rss = rss_mib();
            let lines = session.scrollback.lock().unwrap().lines();

            eprintln!(
                "idle session: {idle_ticks} ticks = {idle:.3}% of one core over {seconds} s, \
                 rss {rss:.1} MiB, scrollback {lines} lines"
            );
            println!("idle_clk_tck={hz:.0}");
            println!("idle_sample_seconds={seconds}");
            println!("idle_baseline_ticks={baseline_ticks}");
            println!("idle_session_ticks={idle_ticks}");
            println!("idle_baseline_pct={baseline:.3}");
            println!("idle_session_pct={idle:.3}");
            println!("idle_session_delta_pct={:.3}", (idle - baseline).max(0.0));
            println!("idle_rss_mib={rss:.1}");
            println!("idle_scrollback_lines={lines}");

            if state != SessionState::Connected {
                eprintln!("session left `connected` during sampling ({state:?}); number is invalid");
                let _ = session.send(Command::Destroy).await;
                session.abort();
                return ExitCode::from(6);
            }
            println!("session_state=connected");

            let _ = session.send(Command::Destroy).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            session.abort();
            ExitCode::SUCCESS
        })
    }
}

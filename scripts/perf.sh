#!/usr/bin/env bash
# Measure the Sentinel resource baselines recorded in docs/perf.md (tasks 3.5 / 10.2).
#
# Runs entirely on the local Linux host against the throwaway sshd fixture, so it needs
# no Windows machine. Numbers that can only come from a real Windows box (cold-start
# memory, ten sessions, GUI frame rate) are printed as `待真机` rows instead of guessed.
#
#   ./scripts/perf.sh                  # all measurements
#   ./scripts/perf.sh --skip-idle      # throughput + replay only (no live session)
#   ./scripts/perf.sh --seconds 30     # longer idle sample for CPU
#   ./scripts/perf.sh --only replay    # comma separated: throughput,replay,idle
#
# stdout is a markdown table; logs and raw numbers land in target/perf/.
# The sshd fixture needs root (test user + listener); nothing else here does.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT/target/perf"
PERF_MANIFEST="$ROOT/scripts/perf/Cargo.toml"
PERF_TARGET="$ROOT/scripts/perf/target/release/sentinel-perf"
TMUX_SOCKET="sentinel-perf"

IDLE_SECONDS=30
ONLY="throughput,replay,idle"
SKIP_IDLE=0

die() {
  printf 'perf: %s\n' "$*" >&2
  exit 1
}

note() {
  printf 'perf: %s\n' "$*" >&2
}

usage() {
  sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --skip-idle) SKIP_IDLE=1; shift ;;
    --seconds)
      [ $# -ge 2 ] || die "--seconds needs a value"
      IDLE_SECONDS="$2"; shift 2 ;;
    --only)
      [ $# -ge 2 ] || die "--only needs a value"
      ONLY="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
done

if [ "$SKIP_IDLE" -eq 1 ]; then
  case ",$ONLY," in
    *,idle,*) ONLY="${ONLY//idle/}" ;;
  esac
  ONLY="$(printf '%s' "$ONLY" | tr ',' '\n' | grep -v '^$' | paste -sd, -)"
fi

wants() {
  case ",$ONLY," in
    *",$1,"*) return 0 ;;
    *) return 1 ;;
  esac
}

# ---------------------------------------------------------------------------
# prerequisites — fail loudly instead of printing empty cells
# ---------------------------------------------------------------------------

[ -f "$ROOT/Cargo.toml" ] || die "run from the repository (Cargo.toml not found in $ROOT)"

if ! command -v cargo >/dev/null 2>&1; then
  die "cargo not on PATH. If Rust is installed under ~/.cargo: export PATH=\"\$HOME/.cargo/bin:\$PATH\""
fi

command -v awk >/dev/null 2>&1 || die "awk not found"
command -v getconf >/dev/null 2>&1 || die "getconf not found (needed for CLK_TCK)"

if wants idle; then
  command -v tmux >/dev/null 2>&1 || die "idle bench needs tmux on PATH (the session's persistence provider)"
fi

mkdir -p "$OUT_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
ENV_LOG="$OUT_DIR/env-$STAMP.log"
THROUGHPUT_LOG="$OUT_DIR/throughput-$STAMP.log"
REPLAY_LOG="$OUT_DIR/replay-$STAMP.log"
IDLE_LOG="$OUT_DIR/idle-$STAMP.log"

# ---------------------------------------------------------------------------
# fixture
# ---------------------------------------------------------------------------

note "starting the sshd fixture (scripts/test-sshd.sh start)…"
if ! FIXTURE_ENV="$(cd "$ROOT" && ./scripts/test-sshd.sh start 2>"$OUT_DIR/sshd-start-$STAMP.err")"; then
  cat "$OUT_DIR/sshd-start-$STAMP.err" >&2 || true
  die "the sshd fixture did not start. It needs root (test user + listener); check /usr/sbin/sshd and port 2222."
fi
# shellcheck disable=SC1090
eval "$FIXTURE_ENV"
[ -n "${SSHTERM_TEST_HOST:-}" ] || die "the fixture printed no SSHTERM_TEST_HOST; refusing to measure against nothing"
note "fixture ready at $SSHTERM_TEST_HOST:$SSHTERM_TEST_PORT (user $SSHTERM_TEST_USER)"

cleanup() {
  # Only the benchmark's own tmux socket; never touch the real daemon's `sentinel` server.
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# measurements
# ---------------------------------------------------------------------------

read_metric() { # read_metric <file> <key>
  awk -F= -v key="$2" '$1 == key { value = $2 } END { if (value != "") print value }' "$1"
}

need_metric() { # need_metric <file> <key> <what>
  local value
  value="$(read_metric "$1" "$2")"
  [ -n "$value" ] || die "missing metric '$2' from $3 (see $1)"
  printf '%s' "$value"
}

THROUGHPUT_MIB=""
THROUGHPUT_MS=""
THROUGHPUT_BYTES=""

if wants throughput; then
  note "throughput: 10 MiB over one SSH channel (cargo test -p ssh --release --test live)"
  set +e
  cargo test -p ssh --release --test live -- \
    --nocapture --exact throughput_reaches_the_baseline >"$THROUGHPUT_LOG" 2>&1
  THROUGHPUT_STATUS=$?
  set -e
  [ "$THROUGHPUT_STATUS" -eq 0 ] || {
    tail -20 "$THROUGHPUT_LOG" >&2
    die "throughput test failed (exit $THROUGHPUT_STATUS); full log: $THROUGHPUT_LOG"
  }
  LINE="$(grep -E 'throughput: [0-9.]+ MiB/s' "$THROUGHPUT_LOG" | tail -1 || true)"
  [ -n "$LINE" ] || die "the throughput test ran but printed no 'throughput:' line (see $THROUGHPUT_LOG)"
  THROUGHPUT_MIB="$(printf '%s' "$LINE" | sed -E 's/.*throughput: ([0-9.]+) MiB\/s.*/\1/')"
  THROUGHPUT_BYTES="$(printf '%s' "$LINE" | sed -E 's/.*\(([0-9]+) bytes.*/\1/')"
  THROUGHPUT_MS="$(printf '%s' "$LINE" | sed -E 's/.* bytes in ([0-9.]+)ms\).*/\1/')"
  [ "$THROUGHPUT_BYTES" = "10485760" ] || die "expected 10485760 bytes, the test reported $THROUGHPUT_BYTES"
  note "throughput: $THROUGHPUT_MIB MiB/s ($THROUGHPUT_MS ms)"
fi

REPLAY_BIN_LOG="$REPLAY_LOG"
if wants replay; then
  note "replay: building scripts/perf (terminal crate only)…"
  cargo build --release --quiet --manifest-path "$PERF_MANIFEST" >>"$REPLAY_LOG" 2>&1 \
    || die "could not build the replay bench (see $REPLAY_LOG)"
  note "replay: 20k lines appended in 4 KiB PTY-sized chunks, then read back"
  "$PERF_TARGET" replay >>"$REPLAY_LOG" 2>&1 \
    || die "replay bench failed (see $REPLAY_LOG)"
fi

if wants idle; then
  note "idle: building scripts/perf with the session feature (first run compiles the daemon)…"
  cargo build --release --quiet --features session --manifest-path "$PERF_MANIFEST" >>"$IDLE_LOG" 2>&1 \
    || die "could not build the idle bench (see $IDLE_LOG)"
  note "idle: one live session (ssh + tmux), sampling ${IDLE_SECONDS}s"
  "$PERF_TARGET" idle-cpu --seconds "$IDLE_SECONDS" >>"$IDLE_LOG" 2>&1 || {
    tail -20 "$IDLE_LOG" >&2
    die "idle bench failed (see $IDLE_LOG)"
  }
fi

# ---------------------------------------------------------------------------
# environment + markdown table
# ---------------------------------------------------------------------------

KERNEL="$(uname -sr)"
CPUS="$(nproc)"
MEM_MIB="$(awk '/MemTotal/ { printf "%d", $2 / 1024 }' /proc/meminfo)"
RUSTC_VERSION="$(rustc --version 2>/dev/null | awk '{ print $2 }' || true)"
TMUX_VERSION="$(tmux -V 2>/dev/null || echo none)"
CPU_MODEL="$(awk -F: '/model name/ { print $2; exit }' /proc/cpuinfo | sed 's/^ //')"
[ -n "$CPU_MODEL" ] || CPU_MODEL="unknown"
[ -n "$RUSTC_VERSION" ] || RUSTC_VERSION="unknown"

cat >"$ENV_LOG" <<ENV
date_utc=$STAMP
kernel=$KERNEL
cpus=$CPUS
mem_total_mib=$MEM_MIB
cpu_model=$CPU_MODEL
rustc=$RUSTC_VERSION
tmux=$TMUX_VERSION
sshd_host=$SSHTERM_TEST_HOST:$SSHTERM_TEST_PORT
ENV

echo "<!-- generated by scripts/perf.sh on $STAMP -->"
echo
echo "测量主机：${CPUS} vCPU（${CPU_MODEL}）、$((MEM_MIB / 1024)) GiB RAM、${KERNEL}、rustc ${RUSTC_VERSION}、${TMUX_VERSION}。"
echo "远端：同机回环 sshd \`${SSHTERM_TEST_HOST}:${SSHTERM_TEST_PORT}\`；构建 profile：release。"
echo
echo "| 指标 | 实测 | 采集方式 | 备注 |"
echo "| --- | --- | --- | --- |"

if [ -n "$THROUGHPUT_MIB" ]; then
  echo "| SSH 通道吞吐（10 MiB） | **${THROUGHPUT_MIB} MiB/s** | \`cargo test -p ssh --release --test live --exact throughput_reaches_the_baseline\` | ${THROUGHPUT_MS} ms / ${THROUGHPUT_BYTES} B，裸通道上限 |"
fi

if wants replay; then
  echo "| 滚动缓冲写入（20k 行 / $(read_metric "$REPLAY_LOG" scrollback_bytes) B） | $(read_metric "$REPLAY_LOG" scrollback_append_ms) ms（$(read_metric "$REPLAY_LOG" scrollback_append_mib_s) MiB/s） | \`sentinel-perf replay\` | $(read_metric "$REPLAY_LOG" scrollback_chunks) 个 4 KiB 批次 |"
  echo "| 回放延迟：attach 回放 2000 行 | 中位 $(read_metric "$REPLAY_LOG" replay_attach_median_ms) ms / p95 $(read_metric "$REPLAY_LOG" replay_attach_p95_ms) ms | \`sentinel-perf replay\` | 200 次，\`Scrollback::tail_lines\` |"
  echo "| 回放延迟：上翻 20k 行到最早（10 页 × 2000 行） | 中位 $(read_metric "$REPLAY_LOG" replay_walk_median_ms) ms / p95 $(read_metric "$REPLAY_LOG" replay_walk_p95_ms) ms | \`sentinel-perf replay\` | 50 次，\`Scrollback::page_before\` |"
  echo "| 回放延迟：resync 全量 20k 行 | 中位 $(read_metric "$REPLAY_LOG" replay_resync_median_ms) ms / p95 $(read_metric "$REPLAY_LOG" replay_resync_p95_ms) ms | \`sentinel-perf replay\` | 50 次，\`Scrollback::since(0)\` |"
fi

if wants idle; then
  echo "| 空闲单会话 CPU（daemon 进程增量） | **$(read_metric "$IDLE_LOG" idle_session_delta_pct)%** 单核 | \`sentinel-perf idle-cpu --seconds ${IDLE_SECONDS}\` | 采样 ${IDLE_SECONDS}s；本进程 $(read_metric "$IDLE_LOG" idle_session_ticks) tick，空转基线 $(read_metric "$IDLE_LOG" idle_baseline_ticks) tick（CLK_TCK=$(read_metric "$IDLE_LOG" idle_clk_tck)） |"
  echo "| 空闲单会话常驻内存（进程 RSS） | $(read_metric "$IDLE_LOG" idle_rss_mib) MiB | \`sentinel-perf idle-cpu\` | 含 bench 进程本身，**不是**产品冷启动内存 |"
fi

echo "| 冷启动内存（daemon + GUI 全部进程） | 待真机 | 人工 | Windows 上 Electron/WebView 进程模型不同，Linux 数值无意义 |"
echo "| 10 个会话 CPU / 内存 | 待真机 | 人工 | 需要 10 个真实标签与 GUI 渲染，见 \`docs/reports/acceptance.md\` A6 |"
echo "| GUI 渲染帧率 / 输入延迟 | 待真机 | 人工 | 只有 Windows + 真实 GPU 的 WebGL 路径有意义 |"
echo
echo "原始数据：\`target/perf/\`（\`env-$STAMP.log\`、\`throughput-$STAMP.log\`、\`replay-$STAMP.log\`、\`idle-$STAMP.log\`）。"

note "done; markdown table on stdout, raw logs in target/perf/"

#!/usr/bin/env bash
# Provision and control a throwaway sshd used by integration tests.
# Requires root (user creation + /run/sshd). Safe to re-run: state lives in $ROOT.
#
#   ./scripts/test-sshd.sh start   # create user/keys, start sshd, print env
#   ./scripts/test-sshd.sh drop    # kill live connections, keep the listener
#   ./scripts/test-sshd.sh stop    # stop sshd (leaves user and files)
#   ./scripts/test-sshd.sh clean   # stop and remove user + files
set -euo pipefail

ROOT="${SSHTERM_SSHD_DIR:-/tmp/sshterm-sshd}"
PORT="${SSHTERM_SSHD_PORT:-2222}"
TEST_USER="${SSHTERM_SSHD_USER:-sshtermtest}"
TEST_PASSWORD="${SSHTERM_SSHD_PASSWORD:-sshtermtest-pw}"
SSHD="${SSHD_BIN:-/usr/sbin/sshd}"

as_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "must run as root (creates a test user and binds sshd)" >&2
    exit 1
  fi
}

start() {
  as_root
  mkdir -p "$ROOT" /run/sshd
  chmod 755 "$ROOT"

  if ! id -u "$TEST_USER" >/dev/null 2>&1; then
    useradd -m -s /bin/bash "$TEST_USER"
  fi
  echo "$TEST_USER:$TEST_PASSWORD" | chpasswd

  [ -f "$ROOT/host_key" ] || ssh-keygen -q -t ed25519 -N '' -f "$ROOT/host_key"
  [ -f "$ROOT/client_key" ] || ssh-keygen -q -t ed25519 -N '' -f "$ROOT/client_key"
  cp "$ROOT/client_key.pub" "$ROOT/authorized_keys"
  chmod 644 "$ROOT/authorized_keys"

  cat > "$ROOT/sshd_config" <<CONF
Port $PORT
ListenAddress 127.0.0.1
HostKey $ROOT/host_key
PidFile $ROOT/sshd.pid
PasswordAuthentication yes
KbdInteractiveAuthentication yes
PubkeyAuthentication yes
AuthorizedKeysFile $ROOT/authorized_keys
PermitRootLogin no
UsePAM yes
StrictModes no
LogLevel ERROR
Subsystem sftp internal-sftp
CONF

  # Guarantee a single listener: stale pid files and duplicate instances made the
  # fixture unreliable.
  pkill -f "sshd -f $ROOT/sshd_config" 2>/dev/null || true
  rm -f "$ROOT/sshd.pid"
  sleep 0.2
  if false; then
    :
  else
    "$SSHD" -f "$ROOT/sshd_config" -E "$ROOT/sshd.log"
    for _ in $(seq 1 50); do
      [ -S "/run/sshd" ] || true
      if (exec 3<>"/dev/tcp/127.0.0.1/$PORT") >/dev/null 2>&1; then exec 3>&-; break; fi
      sleep 0.1
    done
  fi

  # Dedicated agent for the Agent-auth test, isolated from the caller's environment.
  if [ -f "$ROOT/agent.env" ]; then
    # shellcheck disable=SC1090
    . "$ROOT/agent.env"
  fi
  if [ -z "${SSH_AUTH_SOCK:-}" ] || ! ssh-add -l >/dev/null 2>&1; then
    eval "$(ssh-agent -s)" >/dev/null
    ssh-add "$ROOT/client_key" >/dev/null 2>&1
    printf 'export SSH_AUTH_SOCK=%s\nexport SSH_AGENT_PID=%s\n' "$SSH_AUTH_SOCK" "$SSH_AGENT_PID" > "$ROOT/agent.env"
  fi

  fingerprint="$(ssh-keygen -lf "$ROOT/host_key.pub" 2>/dev/null | awk '{print $2}' || true)"
  cat <<ENV
export SSHTERM_TEST_HOST=127.0.0.1
export SSHTERM_TEST_PORT=$PORT
export SSHTERM_TEST_USER=$TEST_USER
export SSHTERM_TEST_PASSWORD=$TEST_PASSWORD
export SSHTERM_TEST_KEY=$ROOT/client_key
export SSHTERM_TEST_HOSTKEY=$fingerprint
export SSH_AUTH_SOCK=$SSH_AUTH_SOCK
ENV
}

# Sever every live connection for the test user without touching the listener.
drop() {
  pkill -9 -f "sshd: $TEST_USER" 2>/dev/null || true
  sleep 0.2
}

stop() {
  if [ -f "$ROOT/sshd.pid" ]; then
    kill "$(cat "$ROOT/sshd.pid")" 2>/dev/null || true
    rm -f "$ROOT/sshd.pid"
  fi
  pkill -f "sshd -f $ROOT/sshd_config" 2>/dev/null || true
  sleep 0.2
}

clean() {
  stop
  id -u "$TEST_USER" >/dev/null 2>&1 && userdel -r "$TEST_USER" 2>/dev/null || true
  rm -rf "$ROOT"
}

case "${1:-start}" in
  start) start ;;
  drop) drop ;;
  stop) stop ;;
  clean) clean ;;
  *) echo "usage: $0 start|drop|stop|clean" >&2; exit 2 ;;
esac

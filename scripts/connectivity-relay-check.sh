#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TFSCALE_CHECK_DIR:-$ROOT_DIR/.tmp/connectivity-relay}"
CONTROL_URL="${TFSCALE_CONTROL_URL:-http://127.0.0.1:8080}"
CONTROL_LISTEN="${TFSCALE_CONTROL_LISTEN:-127.0.0.1:8080}"
UDP_PROBE_LISTEN="${TFSCALE_UDP_PROBE_LISTEN:-127.0.0.1:3478}"
RELAY_LISTEN="${TFSCALE_RELAY_LISTEN:-127.0.0.1:9443}"
RELAY_URL="${TFSCALE_RELAY_URL:-tcp://$RELAY_LISTEN}"
DB_PATH="${TFSCALE_DB_PATH:-$WORK_DIR/tf-scale.db}"
STATE_DIR="${TFSCALE_STATE_DIR:-$WORK_DIR/agent-state}"
LOG_DIR="$WORK_DIR/logs"

TF_SCALED="$ROOT_DIR/target/debug/tfscaled"
TF_SCALECTL="$ROOT_DIR/target/debug/tfscalectl"
TF_SCALE_AGENT="$ROOT_DIR/target/debug/tfscale-agent"
TF_SCALE_RELAY="$ROOT_DIR/target/debug/tfscale-relay"

usage() {
  cat <<EOF
tf-scale connectivity and relay validation

Usage:
  $0 preflight
  $0 build
  $0 control
  $0 relay
  $0 make-key
  $0 agent --login-key <key> [--state-dir <dir>] [--control-url <url>]
  $0 status [--state-dir <dir>]
  $0 ping --target <overlay-ip>
  $0 cleanup [--state-dir <dir>]

Typical two-host relay test:
  # Control/relay host
  TFSCALE_RELAY_URL=tcp://<control-host>:9443 $0 control
  $0 relay
  $0 make-key
  $0 make-key

  # Agent hosts
  sudo TFSCALE_CONTROL_URL=http://<control-host>:8080 $0 agent --login-key <key-a>
  sudo TFSCALE_CONTROL_URL=http://<control-host>:8080 $0 agent --login-key <key-b>

  # Inspect path diagnostics
  $0 status
  ping -c 3 <peer-overlay-ip>
EOF
}

log() {
  printf '== %s ==\n' "$*" >&2
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

preflight() {
  [ "$(uname -s)" = "Linux" ] || fail "this validation must run on Linux"
  [ -e /dev/net/tun ] || fail "missing /dev/net/tun"
  need_cmd cargo
  need_cmd curl
  need_cmd ip
  if [ "$(id -u)" -ne 0 ]; then
    printf 'warning: agent TUN setup requires root or CAP_NET_ADMIN\n' >&2
  fi
  log "preflight ok"
}

build_bins() {
  preflight
  cargo build --workspace
}

start_control() {
  mkdir -p "$LOG_DIR"
  build_bins
  log "starting control on $CONTROL_LISTEN with relay $RELAY_URL"
  "$TF_SCALED" serve \
    --db "$DB_PATH" \
    --listen "$CONTROL_LISTEN" \
    --udp-probe-listen "$UDP_PROBE_LISTEN" \
    --relay-url "$RELAY_URL" >"$LOG_DIR/tfscaled.log" 2>&1 &
  echo $! >"$WORK_DIR/tfscaled.pid"
  sleep 1
  curl -fsS "$CONTROL_URL/healthz" >/dev/null
  log "control ready"
}

start_relay() {
  mkdir -p "$LOG_DIR"
  build_bins
  log "starting relay on $RELAY_LISTEN"
  "$TF_SCALE_RELAY" serve --listen "$RELAY_LISTEN" >"$LOG_DIR/tfscale-relay.log" 2>&1 &
  echo $! >"$WORK_DIR/tfscale-relay.pid"
  sleep 1
  log "relay started"
}

make_key() {
  build_bins
  "$TF_SCALECTL" auth-key create --control-url "$CONTROL_URL"
}

run_agent() {
  local login_key=""
  local state_dir="$STATE_DIR"
  local control_url="$CONTROL_URL"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --login-key) login_key="${2:-}"; shift 2 ;;
      --state-dir) state_dir="${2:-}"; shift 2 ;;
      --control-url) control_url="${2:-}"; shift 2 ;;
      *) fail "unknown agent argument: $1" ;;
    esac
  done
  [ -n "$login_key" ] || fail "missing --login-key"
  mkdir -p "$LOG_DIR"
  build_bins
  TFSCALE_STATE_DIR="$state_dir" "$TF_SCALE_AGENT" up \
    --login-key "$login_key" \
    --control-url "$control_url" >"$LOG_DIR/tfscale-agent.log" 2>&1 &
  echo $! >"$state_dir.pid"
  log "agent started with state $state_dir"
}

status() {
  local state_dir="$STATE_DIR"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --state-dir) state_dir="${2:-}"; shift 2 ;;
      *) fail "unknown status argument: $1" ;;
    esac
  done
  TFSCALE_STATE_DIR="$state_dir" "$TF_SCALE_AGENT" status --json
}

ping_target() {
  local target=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --target) target="${2:-}"; shift 2 ;;
      *) fail "unknown ping argument: $1" ;;
    esac
  done
  [ -n "$target" ] || fail "missing --target"
  ping -c 3 "$target"
}

cleanup() {
  local state_dir="$STATE_DIR"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --state-dir) state_dir="${2:-}"; shift 2 ;;
      *) fail "unknown cleanup argument: $1" ;;
    esac
  done
  for pid_file in "$WORK_DIR/tfscaled.pid" "$WORK_DIR/tfscale-relay.pid" "$state_dir.pid"; do
    if [ -f "$pid_file" ]; then
      kill "$(cat "$pid_file")" >/dev/null 2>&1 || true
      rm -f "$pid_file"
    fi
  done
  TFSCALE_STATE_DIR="$state_dir" "$TF_SCALE_AGENT" down >/dev/null 2>&1 || true
  log "cleanup complete"
}

cmd="${1:-}"
shift || true
case "$cmd" in
  preflight) preflight ;;
  build) build_bins ;;
  control) start_control ;;
  relay) start_relay ;;
  make-key) make_key ;;
  agent) run_agent "$@" ;;
  status) status "$@" ;;
  ping) ping_target "$@" ;;
  cleanup) cleanup "$@" ;;
  -h|--help|"") usage ;;
  *) usage; fail "unknown command: $cmd" ;;
esac

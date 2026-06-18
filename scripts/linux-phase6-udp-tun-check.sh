#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TFSCALE_CHECK_DIR:-$ROOT_DIR/.tmp/linux-phase6}"
CONTROL_URL="${TFSCALE_CONTROL_URL:-http://127.0.0.1:8080}"
CONTROL_LISTEN="${TFSCALE_CONTROL_LISTEN:-127.0.0.1:8080}"
DB_PATH="${TFSCALE_DB_PATH:-$WORK_DIR/tf-scale.db}"
STATE_DIR="${TFSCALE_STATE_DIR:-$WORK_DIR/agent-state}"
LOG_DIR="$WORK_DIR/logs"

TF_SCALED="$ROOT_DIR/target/debug/tfscaled"
TF_SCALECTL="$ROOT_DIR/target/debug/tfscalectl"
TF_SCALE_AGENT="$ROOT_DIR/target/debug/tfscale-agent"

usage() {
  cat <<EOF
tf-scale Linux Phase 6 TUN/UDP validation

Usage:
  $0 preflight
  $0 build
  $0 control
  $0 make-key
  $0 agent --login-key <key> [--state-dir <dir>] [--control-url <url>]
  $0 status [--state-dir <dir>]
  $0 ping --target <overlay-ip>
  $0 cleanup [--state-dir <dir>]
  $0 single-agent

Typical two-host test:
  # Control host
  $0 control
  $0 make-key
  $0 make-key

  # Agent host A, as root or with CAP_NET_ADMIN
  sudo TFSCALE_CONTROL_URL=http://<control-host>:8080 $0 agent --login-key <key-a>

  # Agent host B, as root or with CAP_NET_ADMIN
  sudo TFSCALE_CONTROL_URL=http://<control-host>:8080 $0 agent --login-key <key-b>

  # On each agent host, find peer overlay IP from control/device list, then:
  ping -c 3 <peer-overlay-ip>

Single-host note:
  The agent currently uses interface tfscale0, so full two-agent ping testing
  needs two Linux hosts or separate network namespaces with distinct interfaces.
  Use "single-agent" to validate local TUN, UDP bind, endpoint heartbeat, and
  transport task startup on one host.
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
  [ -e /dev/net/tun ] || fail "missing /dev/net/tun; load tun module or pass --device /dev/net/tun"
  need_cmd ip
  need_cmd cargo
  need_cmd curl
  if [ "$(id -u)" -ne 0 ]; then
    printf 'warning: not root; agent TUN setup requires root or CAP_NET_ADMIN\n' >&2
  fi
  log "preflight ok"
}

build_bins() {
  preflight
  log "building workspace"
  (cd "$ROOT_DIR" && cargo build --workspace)
}

wait_http() {
  local url="$1"
  local tries="${2:-50}"
  for _ in $(seq 1 "$tries"); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done
  return 1
}

start_control() {
  build_bins
  mkdir -p "$LOG_DIR"
  rm -f "$DB_PATH"

  log "starting control plane on $CONTROL_LISTEN"
  "$TF_SCALED" serve --db "$DB_PATH" --listen "$CONTROL_LISTEN" \
    >"$LOG_DIR/tfscaled.log" 2>&1 &
  echo "$!" >"$WORK_DIR/tfscaled.pid"

  wait_http "$CONTROL_URL/healthz" || {
    cat "$LOG_DIR/tfscaled.log" >&2 || true
    fail "control plane did not become healthy"
  }

  log "control plane ready"
  log "logs: $LOG_DIR/tfscaled.log"
}

make_key() {
  [ -x "$TF_SCALECTL" ] || build_bins
  log "creating auth key at $CONTROL_URL"
  TFSCALE_CONTROL_URL="$CONTROL_URL" "$TF_SCALECTL" auth-key create
}

parse_agent_args() {
  LOGIN_KEY="${LOGIN_KEY:-}"
  TARGET_IP="${TARGET_IP:-}"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --login-key)
        LOGIN_KEY="${2:-}"
        shift 2
        ;;
      --state-dir)
        STATE_DIR="${2:-}"
        shift 2
        ;;
      --control-url)
        CONTROL_URL="${2:-}"
        shift 2
        ;;
      --target)
        TARGET_IP="${2:-}"
        shift 2
        ;;
      *)
        fail "unknown argument: $1"
        ;;
    esac
  done
}

start_agent() {
  parse_agent_args "$@"
  [ -n "$LOGIN_KEY" ] || fail "--login-key is required"
  build_bins
  mkdir -p "$LOG_DIR"

  if [ "$(id -u)" -ne 0 ]; then
    fail "agent mode must run as root or with CAP_NET_ADMIN; try sudo"
  fi

  log "cleaning old tfscale0 route/link if present"
  ip route del 100.64.0.0/10 dev tfscale0 2>/dev/null || true
  ip link del tfscale0 2>/dev/null || true
  rm -rf "$STATE_DIR"

  log "starting agent with state dir $STATE_DIR"
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" up \
    --login-key "$LOGIN_KEY" \
    --control-url "$CONTROL_URL" \
    >"$LOG_DIR/agent.log" 2>&1 &
  echo "$!" >"$WORK_DIR/agent.pid"

  wait_for_agent_ready
  print_agent_status

  log "agent is running; logs: $LOG_DIR/agent.log"
  log "stop with: kill $(cat "$WORK_DIR/agent.pid")"
}

wait_for_agent_ready() {
  log "waiting for agent readiness"
  for _ in $(seq 1 80); do
    if ip link show tfscale0 >/dev/null 2>&1 &&
       TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" status 2>/dev/null |
         grep -q 'tun_configured=true'; then
      return 0
    fi
    if [ -f "$LOG_DIR/agent.log" ] && grep -qE 'error|panicked|Operation not permitted' "$LOG_DIR/agent.log"; then
      break
    fi
    sleep 0.5
  done

  cat "$LOG_DIR/agent.log" >&2 || true
  fail "agent did not report tun_configured=true"
}

print_agent_status() {
  log "agent status"
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" status || true

  log "agent status json"
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" status --json || true

  log "interface"
  ip addr show tfscale0 || true

  log "overlay route"
  ip route show 100.64.0.0/10 || true

  log "control devices"
  TFSCALE_CONTROL_URL="$CONTROL_URL" "$TF_SCALECTL" device list || true
}

ping_target() {
  parse_agent_args "$@"
  [ -n "$TARGET_IP" ] || fail "--target is required"
  log "pinging $TARGET_IP"
  ping -c "${PING_COUNT:-3}" "$TARGET_IP"
  print_agent_status
}

cleanup() {
  parse_agent_args "$@"
  log "cleaning validation resources"

  if [ -f "$WORK_DIR/agent.pid" ]; then
    kill "$(cat "$WORK_DIR/agent.pid")" 2>/dev/null || true
    rm -f "$WORK_DIR/agent.pid"
  fi
  if [ -f "$WORK_DIR/tfscaled.pid" ]; then
    kill "$(cat "$WORK_DIR/tfscaled.pid")" 2>/dev/null || true
    rm -f "$WORK_DIR/tfscaled.pid"
  fi

  ip route del 100.64.0.0/10 dev tfscale0 2>/dev/null || true
  ip link del tfscale0 2>/dev/null || true
  rm -rf "$STATE_DIR"
  log "cleanup complete"
}

single_agent() {
  start_control
  local key
  key="$(make_key | tail -n 1)"
  start_agent --login-key "$key"
  log "single-agent validation complete"
  log "This validates TUN setup, UDP bind, heartbeat endpoint publication, and runtime startup."
  log "Run '$0 cleanup' when done."
}

cmd="${1:-}"
if [ -z "$cmd" ]; then
  usage
  exit 1
fi
shift || true

case "$cmd" in
  preflight) preflight "$@" ;;
  build) build_bins "$@" ;;
  control) start_control "$@" ;;
  make-key) make_key "$@" ;;
  agent) start_agent "$@" ;;
  status) parse_agent_args "$@"; print_agent_status ;;
  ping) ping_target "$@" ;;
  cleanup) cleanup "$@" ;;
  single-agent) single_agent "$@" ;;
  -h|--help|help) usage ;;
  *) usage; fail "unknown command: $cmd" ;;
esac

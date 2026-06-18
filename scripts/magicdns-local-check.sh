#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TFSCALE_MAGICDNS_DIR:-$ROOT_DIR/.tmp/magicdns-local}"
CONTROL_URL="${TFSCALE_CONTROL_URL:-http://127.0.0.1:8080}"
CONTROL_LISTEN="${TFSCALE_CONTROL_LISTEN:-127.0.0.1:8080}"
DNS_LISTEN="${TFSCALE_DNS_LISTEN:-127.0.0.1:1053}"
DB_PATH="${TFSCALE_DB_PATH:-$WORK_DIR/tf-scale.db}"
STATE_DIR="${TFSCALE_STATE_DIR:-$WORK_DIR/agent-state}"
LOG_DIR="$WORK_DIR/logs"

TF_SCALED="$ROOT_DIR/target/debug/tfscaled"
TF_SCALECTL="$ROOT_DIR/target/debug/tfscalectl"
TF_SCALE_AGENT="$ROOT_DIR/target/debug/tfscale-agent"

usage() {
  cat <<EOF
tf-scale MagicDNS local proxy validation

Usage:
  $0 preflight
  $0 build
  $0 control
  $0 make-key
  $0 agent --login-key <key> [--dns-listen <addr:port>]
  $0 status
  $0 doctor
  $0 dns-status
  sudo $0 install-dns
  sudo $0 uninstall-dns
  $0 records
  $0 resolve --name <hostname.mesh> [--expect <100.64.0.x>]
  $0 resolve-first
  $0 ping-name --name <hostname.mesh> [--count 3]
  sudo $0 system-resolver-smoke
  sudo $0 service-smoke --login-key <key>
  $0 cleanup

Typical flow:
  $0 control
  key="$($0 make-key | tail -n 1)"
  sudo $0 agent --login-key "$key"
  $0 records
  $0 resolve-first
  sudo $0 system-resolver-smoke
  sudo $0 service-smoke --login-key "$key"
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
  need_cmd dig
  if [ "$(id -u)" -ne 0 ]; then
    printf 'warning: agent mode requires root or CAP_NET_ADMIN for TUN setup\n' >&2
  fi
  log "preflight ok"
}

build_bins() {
  preflight
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

  log "starting control on $CONTROL_LISTEN"
  "$TF_SCALED" serve --db "$DB_PATH" --listen "$CONTROL_LISTEN" \
    >"$LOG_DIR/tfscaled.log" 2>&1 &
  echo "$!" >"$WORK_DIR/tfscaled.pid"

  wait_http "$CONTROL_URL/healthz" || {
    cat "$LOG_DIR/tfscaled.log" >&2 || true
    fail "control plane did not become healthy"
  }
  log "control ready"
}

make_key() {
  [ -x "$TF_SCALECTL" ] || build_bins
  "$TF_SCALECTL" --control-url "$CONTROL_URL" auth-key create
}

start_agent() {
  local login_key=""
  local dns_listen="$DNS_LISTEN"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --login-key) login_key="${2:-}"; shift 2 ;;
      --dns-listen) dns_listen="${2:-}"; shift 2 ;;
      *) fail "unknown agent argument: $1" ;;
    esac
  done
  [ -n "$login_key" ] || fail "missing --login-key"
  build_bins
  mkdir -p "$LOG_DIR"

  log "starting agent with DNS listener $dns_listen"
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" up \
    --login-key "$login_key" \
    --control-url "$CONTROL_URL" \
    --dns-listen "$dns_listen" >"$LOG_DIR/tfscale-agent.log" 2>&1 &
  echo "$!" >"$WORK_DIR/tfscale-agent.pid"

  log "waiting for DNS listener"
  for _ in $(seq 1 60); do
    if TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" status --json 2>/dev/null |
       grep -A5 '"dns"' |
       grep -q '"healthy": true'; then
      log "agent DNS status is healthy"
      return 0
    fi
    sleep 0.5
  done

  cat "$LOG_DIR/tfscale-agent.log" >&2 || true
  fail "agent DNS listener did not become healthy"
}

status() {
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" status --json
}

doctor() {
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" doctor
}

dns_status() {
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" dns status
}

install_dns() {
  [ "$(id -u)" -eq 0 ] || fail "install-dns must run as root because it writes system resolver config"
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" dns install
  dns_status
}

uninstall_dns() {
  [ "$(id -u)" -eq 0 ] || fail "uninstall-dns must run as root because it removes system resolver config"
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" dns uninstall
  dns_status
}

records() {
  "$TF_SCALECTL" --control-url "$CONTROL_URL" dns records
}

first_record() {
  local record
  record="$("$TF_SCALECTL" --control-url "$CONTROL_URL" dns records |
    awk 'NR == 2 { print $2 " " $4 }')"
  [ -n "$record" ] || fail "no DNS records found; wait for agent sync and run records"
  printf '%s\n' "$record"
}

resolve_name() {
  local name=""
  local expected=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --name) name="${2:-}"; shift 2 ;;
      --expect) expected="${2:-}"; shift 2 ;;
      *) fail "unknown resolve argument: $1" ;;
    esac
  done
  [ -n "$name" ] || fail "missing --name"

  local host port answer
  host="${DNS_LISTEN%:*}"
  port="${DNS_LISTEN##*:}"
  answer="$(dig @"$host" -p "$port" "$name" A +short | tail -n 1)"
  printf '%s\n' "$answer"

  if [ -n "$expected" ] && [ "$answer" != "$expected" ]; then
    printf 'hint: run "%s doctor" for agent and MagicDNS diagnostics\n' "$0" >&2
    fail "expected $name to resolve to $expected, got ${answer:-<empty>}"
  fi
}

resolve_first() {
  local name expected
  read -r name expected < <(first_record)
  log "resolving first DNS record: $name -> $expected"
  resolve_name --name "$name" --expect "$expected"
}

ping_name() {
  local name=""
  local count="3"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --name) name="${2:-}"; shift 2 ;;
      --count) count="${2:-}"; shift 2 ;;
      *) fail "unknown ping-name argument: $1" ;;
    esac
  done
  [ -n "$name" ] || fail "missing --name"

  ping -c "$count" "$name" || {
    printf 'hint: run "%s doctor" and "%s dns-status" for resolver diagnostics\n' "$0" "$0" >&2
    fail "ping failed for $name"
  }
}

system_resolver_smoke() {
  [ "$(id -u)" -eq 0 ] || fail "system-resolver-smoke must run as root because it installs system resolver config"
  local name expected
  read -r name expected < <(first_record)

  resolve_name --name "$name" --expect "$expected"
  install_dns
  trap 'TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" dns uninstall >/dev/null 2>&1 || true' EXIT
  ping_name --name "$name"
  uninstall_dns
  trap - EXIT
}

service_smoke() {
  [ "$(id -u)" -eq 0 ] || fail "service-smoke must run as root because it installs a system service"
  local login_key=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --login-key) login_key="${2:-}"; shift 2 ;;
      *) fail "unknown service-smoke argument: $1" ;;
    esac
  done
  [ -n "$login_key" ] || fail "missing --login-key"

  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" service plan \
    --login-key "$login_key" \
    --control-url "$CONTROL_URL" \
    --dns-listen "$DNS_LISTEN"
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" service install \
    --login-key "$login_key" \
    --control-url "$CONTROL_URL" \
    --dns-listen "$DNS_LISTEN"
  trap 'TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" service uninstall >/dev/null 2>&1 || true' EXIT
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" service status
  systemctl restart tfscale-agent
  sleep 2
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" status --json
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" service uninstall
  trap - EXIT
}

cleanup() {
  if [ "$(id -u)" -eq 0 ] && [ -x "$TF_SCALE_AGENT" ]; then
    TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" dns uninstall >/dev/null 2>&1 || true
    TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" service uninstall >/dev/null 2>&1 || true
  fi
  for pid_file in "$WORK_DIR/tfscaled.pid" "$WORK_DIR/tfscale-agent.pid"; do
    if [ -f "$pid_file" ]; then
      kill "$(cat "$pid_file")" >/dev/null 2>&1 || true
      rm -f "$pid_file"
    fi
  done
  TFSCALE_STATE_DIR="$STATE_DIR" "$TF_SCALE_AGENT" down >/dev/null 2>&1 || true
  log "cleanup complete"
}

cmd="${1:-}"
shift || true
case "$cmd" in
  preflight) preflight ;;
  build) build_bins ;;
  control) start_control ;;
  make-key) make_key ;;
  agent) start_agent "$@" ;;
  status) status ;;
  doctor) doctor ;;
  dns-status) dns_status ;;
  install-dns) install_dns ;;
  uninstall-dns) uninstall_dns ;;
  records) records ;;
  resolve) resolve_name "$@" ;;
  resolve-first) resolve_first ;;
  ping-name) ping_name "$@" ;;
  system-resolver-smoke) system_resolver_smoke ;;
  service-smoke) service_smoke "$@" ;;
  cleanup) cleanup ;;
  -h|--help|"") usage ;;
  *) usage; fail "unknown command: $cmd" ;;
esac

#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TFSCALE_E2E_DIR:-$ROOT_DIR/.tmp/e2e-control-cli}"
CONTROL_LISTEN="${TFSCALE_CONTROL_LISTEN:-127.0.0.1:18080}"
CONTROL_URL="${TFSCALE_CONTROL_URL:-http://$CONTROL_LISTEN}"
DB_PATH="${TFSCALE_DB_PATH:-$WORK_DIR/tf-scale.db}"
LOG_DIR="$WORK_DIR/logs"

TF_SCALED="$ROOT_DIR/target/debug/tfscaled"
TF_SCALECTL="$ROOT_DIR/target/debug/tfscalectl"

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

cleanup() {
  if [ -f "$WORK_DIR/tfscaled.pid" ]; then
    kill "$(cat "$WORK_DIR/tfscaled.pid")" 2>/dev/null || true
    rm -f "$WORK_DIR/tfscaled.pid"
  fi
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

build_bins() {
  need_cmd cargo
  need_cmd curl
  log "building workspace"
  (cd "$ROOT_DIR" && cargo build --workspace)
}

start_control() {
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
}

run_smoke() {
  build_bins
  start_control

  log "checking health endpoint"
  curl -fsS "$CONTROL_URL/healthz" >/dev/null

  log "creating auth key"
  local key
  key="$(TFSCALE_CONTROL_URL="$CONTROL_URL" "$TF_SCALECTL" auth-key create)"
  case "$key" in
    tfk_*) ;;
    *) fail "auth key did not use expected tfk_ prefix: $key" ;;
  esac

  log "listing devices"
  TFSCALE_CONTROL_URL="$CONTROL_URL" "$TF_SCALECTL" device list

  log "smoke validation passed"
  printf 'control_url=%s\n' "$CONTROL_URL"
  printf 'auth_key_prefix=%s\n' "${key:0:4}"
  printf 'logs=%s\n' "$LOG_DIR/tfscaled.log"
}

usage() {
  cat <<EOF
tf-scale Phase 7 control/CLI smoke test

Usage:
  $0

Environment:
  TFSCALE_E2E_DIR        temp directory, default: .tmp/e2e-control-cli
  TFSCALE_CONTROL_LISTEN listen address, default: 127.0.0.1:18080
  TFSCALE_CONTROL_URL    control URL, default: http://127.0.0.1:18080
  TFSCALE_DB_PATH        sqlite db path, default: <temp>/tf-scale.db
EOF
}

case "${1:-}" in
  -h|--help|help)
    usage
    ;;
  "")
    trap cleanup EXIT
    run_smoke
    ;;
  *)
    usage
    fail "unknown argument: $1"
    ;;
esac

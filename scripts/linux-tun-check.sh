#!/usr/bin/env sh
set -eu

echo "== tf-scale Linux TUN preflight =="

if [ "$(uname -s)" != "Linux" ]; then
  echo "error: this check must run on Linux" >&2
  exit 1
fi

if [ ! -e /dev/net/tun ]; then
  echo "error: missing /dev/net/tun" >&2
  echo "hint: load the tun module or pass --device /dev/net/tun into the container" >&2
  exit 1
fi

if ! command -v ip >/dev/null 2>&1; then
  echo "error: missing required command: ip" >&2
  echo "hint: install iproute2" >&2
  exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "warning: not running as root; TUN setup requires root or CAP_NET_ADMIN" >&2
fi

echo "preflight ok"
echo
echo "Build:"
echo "  cargo build --workspace"
echo
echo "Run control plane:"
echo "  target/debug/tfscaled serve --db ./tf-scale-dev.db --listen 127.0.0.1:8080"
echo
echo "Create auth key:"
echo "  KEY=\"\$(target/debug/tfscalectl auth-key create)\""
echo
echo "Run agent with TUN setup:"
echo "  sudo TFSCALE_STATE_DIR=./state target/debug/tfscale-agent up --login-key \"\$KEY\" --control-url http://127.0.0.1:8080"
echo
echo "Verify:"
echo "  ip addr show tfscale0"
echo "  ip route show 100.64.0.0/10"

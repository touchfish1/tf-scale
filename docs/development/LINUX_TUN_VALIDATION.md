# Linux TUN Validation

Use this checklist on a Linux host after building `tfscale-agent` and
`tfscaled`. It validates the Linux TUN adapter and, with two Linux hosts, the
Phase 6 UDP data plane.

For the scripted flow, prefer:

```sh
scripts/linux-phase6-udp-tun-check.sh single-agent
```

That validates local TUN setup, UDP bind, endpoint heartbeat publication, and
transport runtime startup on one Linux host. Full overlay ping validation needs
two Linux hosts because the agent currently uses the fixed `tfscale0` interface.

## Host Requirements

- Linux with `/dev/net/tun`.
- `ip` from `iproute2`.
- Root or `CAP_NET_ADMIN`.
- No existing `tfscale0` interface owned by another process.

For containers, add:

```sh
--cap-add NET_ADMIN --device /dev/net/tun
```

## Build

```sh
cargo build --workspace
```

## Scripted Phase 6 Validation

On the control host:

```sh
TFSCALE_CONTROL_LISTEN=0.0.0.0:8080 \
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh control

TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh make-key

TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh make-key
```

On each agent host, use one key:

```sh
sudo TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
  scripts/linux-phase6-udp-tun-check.sh agent --login-key <key>
```

Then ping the peer overlay IP:

```sh
ping -c 3 100.64.0.x
scripts/linux-phase6-udp-tun-check.sh status
```

Expected backend status includes `tun_configured=true`, `udp_bound=true`,
`transport_running=true`, and nonzero `tx_packets` / `rx_packets` after ping.

## Start Control Plane

```sh
rm -f ./tf-scale-dev.db
target/debug/tfscaled serve --db ./tf-scale-dev.db --listen 127.0.0.1:8080
```

In another shell:

```sh
KEY="$(target/debug/tfscalectl auth-key create)"
```

## Start Agent With TUN Setup

```sh
sudo TFSCALE_STATE_DIR=./state \
  target/debug/tfscale-agent up \
  --login-key "$KEY" \
  --control-url http://127.0.0.1:8080
```

Expected behavior:

- Agent registers and remains running.
- Backend status message includes `tun_configured=true`.
- `tfscale0` exists.

## Verify Interface and Route

```sh
ip addr show tfscale0
ip route show 100.64.0.0/10
```

Expected:

- `tfscale0` has the assigned `100.64.0.x/32` address.
- `100.64.0.0/10` routes through `tfscale0`.

## Common Failures

- `missing Linux TUN device: /dev/net/tun`: load the `tun` kernel module or pass
  the TUN device into the container.
- `required command is missing: ip`: install `iproute2`.
- `Operation not permitted`: run with root or `CAP_NET_ADMIN`.
- Route conflict: remove the conflicting route or run on a clean test host.

## Cleanup

Stop the agent with `Ctrl+C`, then remove the interface and route if they remain:

```sh
sudo ip route del 100.64.0.0/10 dev tfscale0 2>/dev/null || true
sudo ip link del tfscale0 2>/dev/null || true
rm -rf ./state ./tf-scale-dev.db
```

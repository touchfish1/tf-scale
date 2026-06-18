# Linux TUN Validation

Use this checklist on a Linux host after building `tfscale-agent` and
`tfscaled`. It validates the Phase 4 TUN adapter without requiring packet
encryption or UDP peer transport.

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

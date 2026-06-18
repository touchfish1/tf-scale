# v0.1 端到端验收指南

这份文档用于验证 v0.1 的最小可用链路：control plane、CLI、agent、Linux
TUN/UDP 数据面和设备删除后的 peer map 收敛。

## 验收层级

建议按顺序执行：

1. 本地 control/CLI 冒烟：不需要 root，不创建 TUN。
2. Linux 单机 agent 冒烟：需要 root 或 `CAP_NET_ADMIN`。
3. 双机 overlay ping：需要两台直接可达的 Linux agent 主机。
4. 设备删除验证：确认删除后的设备不再出现在列表和后续 peer map 中。

## 1. 本地 Control/CLI 冒烟

在开发机执行：

```sh
chmod +x scripts/e2e-control-cli-smoke.sh
scripts/e2e-control-cli-smoke.sh
```

脚本会自动完成：

- `cargo build --workspace`
- 启动临时 `tfscaled`
- 检查 `/healthz`
- 使用 `tfscalectl auth-key create` 创建 key
- 使用 `tfscalectl device list` 访问设备列表
- 退出时清理临时 control 进程

成功时应看到：

```text
== smoke validation passed ==
control_url=http://127.0.0.1:18080
auth_key_prefix=tfk_
```

如果 `18080` 被占用，可以换端口：

```sh
TFSCALE_CONTROL_LISTEN=127.0.0.1:18081 \
TFSCALE_CONTROL_URL=http://127.0.0.1:18081 \
scripts/e2e-control-cli-smoke.sh
```

## 2. Linux 单机 Agent 冒烟

在 Linux 主机上执行：

```sh
git pull
chmod +x scripts/linux-phase6-udp-tun-check.sh
sudo scripts/linux-phase6-udp-tun-check.sh single-agent
```

成功标准：

- `tfscale0` 创建成功。
- `ip route show 100.64.0.0/10` 指向 `tfscale0`。
- agent status 包含 `tun_configured=true`、`udp_bound=true`、
  `transport_running=true`。

清理：

```sh
sudo scripts/linux-phase6-udp-tun-check.sh cleanup
```

更多 Linux TUN 细节见 [Linux TUN 验证指南](LINUX_TUN_VALIDATION.md)。

## 3. 双机 Overlay Ping

准备三类角色：

- control host：运行 `tfscaled`，监听两台 agent 可访问的地址。
- agent host A：运行第一个 `tfscale-agent`。
- agent host B：运行第二个 `tfscale-agent`。

control host：

```sh
TFSCALE_CONTROL_LISTEN=0.0.0.0:8080 \
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh control
```

生成两个 key：

```sh
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh make-key

TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh make-key
```

agent host A 和 B 分别执行，每台使用不同 key：

```sh
sudo TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh agent --login-key <key>
```

查看 overlay IP：

```sh
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
target/debug/tfscalectl device list
```

在 A 上 ping B 的 overlay IP，或在 B 上 ping A 的 overlay IP：

```sh
ping -c 3 100.64.0.x
scripts/linux-phase6-udp-tun-check.sh status
```

成功标准：

- 两台设备拥有不同的 `100.64.0.x`。
- `ping` 可以收到回复。
- ping 后 `tx_packets` 和 `rx_packets` 不是 0。

## 4. 设备删除验证

在 control host 上删除其中一台设备：

```sh
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
target/debug/tfscalectl device delete <device-id>
```

确认设备列表不再包含它：

```sh
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
target/debug/tfscalectl device list
```

等待一个 agent poll 周期后，在存活 agent 上检查：

```sh
scripts/linux-phase6-udp-tun-check.sh status
TFSCALE_STATE_DIR=<agent-state-dir> target/debug/tfscale-agent status --json
```

成功标准：

- `device list` 不再显示被删除设备。
- 存活 agent 的 `status --json` 仍显示本机 `device_id` 和 `ipv4`，backend
  `message` 中的 `transport_peers` / `transport_sessions` 已随 peer map 收敛。
- 对被删除设备 overlay IP 的 ping 不应继续成功。

## 常见问题

- `/dev/net/tun` 不存在：加载 `tun` 模块，或容器中传入
  `--device /dev/net/tun`。
- `Operation not permitted`：使用 root，或授予 `CAP_NET_ADMIN`。
- `tfscale0` 已存在：先执行 `sudo scripts/linux-phase6-udp-tun-check.sh cleanup`。
- control host 不可达：检查监听地址、防火墙和 `TFSCALE_CONTROL_URL`。
- UDP 不通：检查两台 agent 间的防火墙、安全组和路由。
- packet counters 不增长：确认 ping 的目标是对端 overlay IP。

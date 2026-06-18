# macOS TUN/utun 验证指南

这份文档用于在 macOS 主机上验证 Phase 8 的 utun 支持，并与 Linux agent 做
v0.1 parity 验收。

## 主机要求

- macOS 主机。
- 已安装 Rust toolchain。
- agent 需要 `sudo` 权限来创建 utun、配置地址和路由。
- control host 与 macOS/Linux agent 之间 TCP `8080` 和 UDP backend 端口可达。

## 构建

```sh
git pull
cargo build --workspace
```

## 启动 macOS Agent

在 control host 上先启动 control plane，并创建一个 auth key。然后在 macOS
主机执行：

```sh
sudo TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
  target/debug/tfscale-agent --state-dir ./state up --login-key <key>
```

## 验证状态

另开一个终端：

```sh
target/debug/tfscale-agent --state-dir ./state status --json
```

成功时应看到：

- backend `healthy=true`。
- backend message 包含 `tun_configured=true`。
- backend message 包含 `udp_bound=true`。
- backend message 包含 `transport_running=true`。
- `interface_name` 是真实 utun 名，例如 `utun4`。

确认接口和路由：

```sh
ifconfig <utun>
netstat -rn | grep 100.64
```

预期：

- `<utun>` 上有本机 `100.64.0.x` 地址。
- `100.64.0.0/10` 路由指向该 utun。

## Linux/macOS Overlay Ping

Linux agent 和 macOS agent 都注册后，在 control host 查看 overlay IP：

```sh
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
target/debug/tfscalectl device list
```

在 macOS 上 ping Linux overlay IP：

```sh
ping -c 3 <linux-overlay-ip>
```

在 Linux 上 ping macOS overlay IP：

```sh
ping -c 3 <macos-overlay-ip>
```

成功标准：

- 双方拥有不同的 `100.64.0.x`。
- 双向 ping 成功。
- 两端 `status --json` 的 backend message 中 `tx_packets` / `rx_packets` 非 0。

## 清理

停止 agent 后执行：

```sh
sudo target/debug/tfscale-agent --state-dir ./state down
sudo route -n delete -net 100.64.0.0/10 -interface <utun> 2>/dev/null || true
sudo ifconfig <utun> down 2>/dev/null || true
rm -rf ./state
```

utun 设备通常会在进程退出、fd 关闭后自动消失。

## 常见问题

- `Operation not permitted`：使用 `sudo` 运行 agent。
- `ifconfig ... failed`：确认 utun 名来自 `status --json`，不要手写 `tfscale0`。
- `route ... failed`：检查是否已有冲突的 `100.64.0.0/10` 路由。
- ping 不通：检查 macOS 防火墙、Linux 防火墙、云安全组和 UDP 连通性。


# Linux TUN 验证指南

这份文档用于在 Linux 主机上验证 `tfscale-agent` 和 `tfscaled`。单机流程用于确认
TUN 初始化、UDP 监听、心跳上报和 transport runtime 已启动；双机流程用于验证
Phase 6 UDP 数据面可以通过 overlay IP 互相 ping 通。

完整 v0.1 验收流程见 [v0.1 端到端验收指南](E2E_VALIDATION.md)。

优先使用脚本化流程：

```sh
scripts/linux-phase6-udp-tun-check.sh single-agent
```

完整 overlay ping 验证需要两台 Linux 主机，因为当前 agent 固定使用 `tfscale0`
接口，同一台主机上不能直接启动两个独立 agent 进行完整互 ping。

## 主机要求

- Linux 主机，并且存在 `/dev/net/tun`。
- 已安装 `iproute2`，可以使用 `ip` 命令。
- agent 需要 root 或 `CAP_NET_ADMIN` 权限。
- 测试前不要有其他进程占用 `tfscale0`。

如果在容器中测试，需要额外添加：

```sh
--cap-add NET_ADMIN --device /dev/net/tun
```

## 拉取代码并构建

```sh
git pull
cargo build --workspace
```

## 单机冒烟验证

在一台 Linux 主机上执行：

```sh
chmod +x scripts/linux-phase6-udp-tun-check.sh
sudo scripts/linux-phase6-udp-tun-check.sh single-agent
```

成功时，输出中应包含：

```text
tun_configured=true
udp_bound=true
transport_running=true
```

同时确认 `tfscale0` 和 overlay 路由存在：

```sh
ip addr show tfscale0
ip route show 100.64.0.0/10
```

单机冒烟验证不要求 ping 通另一台机器，只验证本机 TUN、UDP、endpoint 心跳和
transport runtime 启动正常。

清理测试资源：

```sh
sudo scripts/linux-phase6-udp-tun-check.sh cleanup
```

## 双机真实 Ping 验证

在控制机上启动 control plane：

```sh
TFSCALE_CONTROL_LISTEN=0.0.0.0:8080 \
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh control
```

在控制机上生成两个 auth key：

```sh
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh make-key

TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
scripts/linux-phase6-udp-tun-check.sh make-key
```

在两台 agent 主机上分别执行，每台使用一个不同的 key：

```sh
sudo TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
  scripts/linux-phase6-udp-tun-check.sh agent --login-key <key>
```

在控制机上查看设备和 overlay IP：

```sh
TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
target/debug/tfscalectl device list
```

然后在一台 agent 主机上 ping 另一台的 `100.64.0.x`：

```sh
ping -c 3 100.64.0.x
scripts/linux-phase6-udp-tun-check.sh status
```

验证通过的标准：

- `ping` 可以收到回复。
- `status` 中包含 `tun_configured=true`、`udp_bound=true` 和
  `transport_running=true`。
- ping 之后 `tx_packets` / `rx_packets` 不是 0。

## 手动启动 Control Plane

```sh
rm -f ./tf-scale-dev.db
target/debug/tfscaled serve --db ./tf-scale-dev.db --listen 127.0.0.1:8080
```

In another shell:

```sh
KEY="$(target/debug/tfscalectl auth-key create)"
```

## 手动启动 Agent 并配置 TUN

```sh
sudo TFSCALE_STATE_DIR=./state \
  target/debug/tfscale-agent up \
  --login-key "$KEY" \
  --control-url http://127.0.0.1:8080
```

预期行为：

- agent 注册成功并持续运行。
- backend 状态包含 `tun_configured=true`。
- `tfscale0` 存在。

## 验证接口和路由

```sh
ip addr show tfscale0
ip route show 100.64.0.0/10
```

预期结果：

- `tfscale0` 拥有分配的 `100.64.0.x/32` 地址。
- `100.64.0.0/10` 路由指向 `tfscale0`。

## 常见失败

- `missing Linux TUN device: /dev/net/tun`：加载 `tun` 内核模块，或把 TUN
  设备传入容器。
- `required command is missing: ip`：安装 `iproute2`。
- `Operation not permitted`：使用 root 运行，或授予 `CAP_NET_ADMIN`。
- 路由冲突：删除已有冲突路由，或换一台干净测试机。

## 清理

如果使用脚本启动，执行：

```sh
sudo scripts/linux-phase6-udp-tun-check.sh cleanup
```

如果手动启动，先用 `Ctrl+C` 停止 agent；如果接口和路由仍存在，再执行：

```sh
sudo ip route del 100.64.0.0/10 dev tfscale0 2>/dev/null || true
sudo ip link del tfscale0 2>/dev/null || true
rm -rf ./state ./tf-scale-dev.db
```

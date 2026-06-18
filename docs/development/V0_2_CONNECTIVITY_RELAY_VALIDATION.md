# v0.2 连接穿透与 Relay 验证指南

本文档用于在 Linux 主机上验证 v0.2 的 endpoint discovery、UDP hole punching
和 relay fallback。当前 relay 是 TCP JSON Lines 原型，payload 仍是 backend 已加密
frame，relay 不需要也不能解密用户 IP 包。

## 环境准备

每台 agent 主机需要 Linux、`/dev/net/tun`、`cargo`、`curl`、`ip` 和 root 或
`CAP_NET_ADMIN` 权限。先在仓库根目录执行：

```sh
scripts/connectivity-relay-check.sh preflight
scripts/connectivity-relay-check.sh build
```

## 启动 Control 与 Relay

在 control/relay 主机上启动控制面和中继：

```sh
TFSCALE_RELAY_URL=tcp://<control-ip>:9443 scripts/connectivity-relay-check.sh control
TFSCALE_RELAY_LISTEN=0.0.0.0:9443 scripts/connectivity-relay-check.sh relay
```

创建两个登录 key：

```sh
TFSCALE_CONTROL_URL=http://<control-ip>:8080 scripts/connectivity-relay-check.sh make-key
TFSCALE_CONTROL_URL=http://<control-ip>:8080 scripts/connectivity-relay-check.sh make-key
```

确认 control 下发 relay metadata：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 relay list
target/debug/tfscalectl --control-url http://<control-ip>:8080 --json relay list
```

## 启动 Agent

在两台 agent 主机上分别执行：

```sh
sudo TFSCALE_CONTROL_URL=http://<control-ip>:8080 \
  scripts/connectivity-relay-check.sh agent --login-key <key-a>

sudo TFSCALE_CONTROL_URL=http://<control-ip>:8080 \
  TFSCALE_STATE_DIR=/tmp/tfscale-agent-b \
  scripts/connectivity-relay-check.sh agent --login-key <key-b>
```

用 `tfscalectl device list` 查看 overlay IP：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 device list
```

## 验证 Direct Path

同 LAN 或常见 cone NAT 下，两个 agent 拿到 peer map 后会先进入快速打洞窗口：
unknown/relay path 会以约 250ms 间隔向所有 UDP endpoint 发送一小段 encrypted
probe burst，成功后切到 `direct`，之后回到低频保活。等待两个 agent 同步 3 到
10 秒后执行：

```sh
sudo TFSCALE_STATE_DIR=<agent-state-dir> scripts/connectivity-relay-check.sh status
ping -c 3 <peer-overlay-ip>
```

`status --json` 中应看到 peer `path` 为 `direct`，`endpoint` 为对端 LAN 或
public UDP 地址，`rtt_ms` 有值且 `failures` 较低。

如果还在快速打洞窗口，backend message 中会包含 `fast_probe_peers=<n>`。
该值降为 `0` 且 peer 仍是 `unknown` 时，说明当前 endpoint 组合可能没有打通。

## 验证 Relay Fallback

如果要强制验证 relay，可以在两端防火墙阻断 peer UDP 数据面端口，或把 agent 放到
symmetric NAT / UDP 受限网络中。等待 direct probe 超时后再次查看状态：

```sh
sudo TFSCALE_STATE_DIR=<agent-state-dir> scripts/connectivity-relay-check.sh status
ping -c 3 <peer-overlay-ip>
```

预期 peer `path` 变为 `relay`，`endpoint` 显示 relay endpoint，ping 仍可通。
此时 direct probe 会继续后台重试；当 UDP direct 恢复后，路径应自动切回
`direct`。

## 常见问题

- `path=unknown`：peer endpoint 还未下发、probe 尚未成功，或 relay metadata 缺失。
- `fast_probe_peers` 持续大于 0：agent 正在对 unknown/relay peer 进行快速打洞。
- `failures` 持续增长：UDP 被防火墙/NAT 阻断，检查两端和 control 的端口。
- `relay list` 为空：control 启动时缺少 `--relay-url`。
- agent 启动失败并提示 TUN 权限：使用 `sudo`，或给二进制授予 `CAP_NET_ADMIN`。
- overlay ping 不通但 `path=relay`：确认 `tfscale-relay` 监听地址可被两端访问。

## 清理

```sh
sudo scripts/connectivity-relay-check.sh cleanup
sudo TFSCALE_STATE_DIR=/tmp/tfscale-agent-b scripts/connectivity-relay-check.sh cleanup
```

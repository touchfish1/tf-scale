# P2P 远程互联 Linux 测试手册

本文档用于在 Linux 主机上验证 tf-scale 的核心目标：两台机器通过 control plane
自动发现 endpoint，优先建立 P2P direct UDP path，并通过 overlay IP 通信。

## 测试目标

成功标准：

- control 能注册两台 agent。
- 两台 agent 都获得 `100.64.0.x` overlay IP。
- 两台 agent 的 `status --json` 能看到对端 peer。
- 常见同 LAN 或 cone NAT 场景下 peer path 变为 `direct`。
- 一台机器可以 `ping` 另一台机器的 overlay IP。

辅助诊断：

- `fast_probe_peers=<n>`：agent 正在快速打洞。
- peer `path=direct`：P2P UDP 已建立。
- peer `path=unknown`：还未打通 direct。
- peer `path=relay`：direct 失败后走 relay fallback。

## 快速执行清单

### 1. Control 主机

```sh
cd /path/to/tf-sacle
git pull
cargo build --workspace

export TFSCALE_CONTROL_LISTEN=0.0.0.0:8080
export TFSCALE_CONTROL_URL=http://<control-ip>:8080
export TFSCALE_UDP_PROBE_LISTEN=0.0.0.0:3478
export TFSCALE_RELAY_LISTEN=0.0.0.0:9443
export TFSCALE_RELAY_URL=tcp://<control-ip>:9443

scripts/connectivity-relay-check.sh control
scripts/connectivity-relay-check.sh relay
curl -fsS http://<control-ip>:8080/healthz
```

### 2. 创建两个登录 Key

```sh
key_a="$(target/debug/tfscalectl --control-url http://<control-ip>:8080 auth-key create | tail -n 1)"
key_b="$(target/debug/tfscalectl --control-url http://<control-ip>:8080 auth-key create | tail -n 1)"

echo "$key_a"
echo "$key_b"
```

### 3. Linux A 启动 Agent

```sh
cd /path/to/tf-sacle
git pull
cargo build --workspace

export TFSCALE_CONTROL_URL=http://<control-ip>:8080
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-a

sudo -E target/debug/tfscale-agent up \
  --login-key "<key-a>" \
  --control-url http://<control-ip>:8080
```

### 4. Linux B 启动 Agent

```sh
cd /path/to/tf-sacle
git pull
cargo build --workspace

export TFSCALE_CONTROL_URL=http://<control-ip>:8080
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-b

sudo -E target/debug/tfscale-agent up \
  --login-key "<key-b>" \
  --control-url http://<control-ip>:8080
```

### 5. 查看 Overlay IP

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 device list
```

记录：

```text
Agent A = 100.64.0.x
Agent B = 100.64.0.y
```

### 6. 检查 P2P Path

在两台 agent 上分别执行：

```sh
sudo -E target/debug/tfscale-agent status --json
```

重点看：

```text
path=direct
direct_peers=1
fast_probe_peers=0
direct_paths=<peer>@<ip>:<port>/rtt=<n>ms
```

### 7. 验证 Overlay 通信

Agent A ping Agent B：

```sh
ping -c 3 <agent-b-overlay-ip>
```

Agent B ping Agent A：

```sh
ping -c 3 <agent-a-overlay-ip>
```

### 8. 失败时立即收集

两台 agent 都执行：

```sh
ip addr show tfscale0
ip route show 100.64.0.0/10
sudo ss -lunp | grep 51820 || true
sudo -E target/debug/tfscale-agent status --json
```

Control 主机执行：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 device list
target/debug/tfscalectl --control-url http://<control-ip>:8080 relay list
sudo ss -lunp | grep 3478 || true
sudo ss -ltnp | grep 8080 || true
sudo ss -ltnp | grep 9443 || true
```

### 9. 测试结果回填模板

```text
Agent A overlay IP:
Agent B overlay IP:
Agent A status peers:
Agent B status peers:
A ping B 结果:
B ping A 结果:
Control relay list:
问题日志:
```

## 主机规划

推荐准备三台 Linux 主机：

- Control：运行 `tfscaled`，两台 agent 都能访问它的 TCP 8080 和 UDP 3478。
- Agent A：第一台被组网机器。
- Agent B：第二台被组网机器。

如果只有两台机器，也可以让 Control 和 Agent A 共用同一台机器。

本文用这些占位符：

```text
<control-ip>     Control 主机可被 Agent A/B 访问的 IP
<key-a>          Agent A 的登录 key
<key-b>          Agent B 的登录 key
<agent-a-ip>     Agent A 的 100.64.0.x overlay IP
<agent-b-ip>     Agent B 的 100.64.0.x overlay IP
```

## 端口要求

Control 主机：

- TCP `8080`：control HTTP API。
- UDP `3478`：public endpoint probe。
- TCP `9443`：relay fallback，可选但建议开。

Agent 主机：

- 需要 `/dev/net/tun`。
- 需要 root 或 `CAP_NET_ADMIN`。
- 需要允许 agent 的 UDP 数据面端口出站。默认 backend listen port 是 `51820`，
  如果使用 `listen_port=0` 的测试场景则是动态端口。

## 1. 拉代码与构建

所有主机进入仓库根目录：

```sh
cd /path/to/tf-sacle
git pull
chmod +x scripts/connectivity-relay-check.sh
```

每台 Linux 主机做预检查：

```sh
scripts/connectivity-relay-check.sh preflight
```

构建：

```sh
scripts/connectivity-relay-check.sh build
```

## 2. 启动 Control

在 Control 主机执行：

```sh
export TFSCALE_CONTROL_LISTEN=0.0.0.0:8080
export TFSCALE_CONTROL_URL=http://<control-ip>:8080
export TFSCALE_UDP_PROBE_LISTEN=0.0.0.0:3478
export TFSCALE_RELAY_LISTEN=0.0.0.0:9443
export TFSCALE_RELAY_URL=tcp://<control-ip>:9443

scripts/connectivity-relay-check.sh control
scripts/connectivity-relay-check.sh relay
```

确认 control 健康：

```sh
curl -fsS http://<control-ip>:8080/healthz
```

确认 relay metadata 下发：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 relay list
```

## 3. 创建登录 Key

在 Control 主机执行：

```sh
export TFSCALE_CONTROL_URL=http://<control-ip>:8080

key_a="$(scripts/connectivity-relay-check.sh make-key | tail -n 1)"
key_b="$(scripts/connectivity-relay-check.sh make-key | tail -n 1)"

printf 'Agent A key: %s\n' "$key_a"
printf 'Agent B key: %s\n' "$key_b"
```

把 `key_a` 复制到 Agent A，把 `key_b` 复制到 Agent B。

## 4. 启动 Agent A

在 Agent A 主机执行：

```sh
cd /path/to/tf-sacle
export TFSCALE_CONTROL_URL=http://<control-ip>:8080
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-a

sudo -E scripts/connectivity-relay-check.sh agent --login-key "<key-a>"
```

查看状态：

```sh
sudo -E scripts/connectivity-relay-check.sh status
```

## 5. 启动 Agent B

在 Agent B 主机执行：

```sh
cd /path/to/tf-sacle
export TFSCALE_CONTROL_URL=http://<control-ip>:8080
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-b

sudo -E scripts/connectivity-relay-check.sh agent --login-key "<key-b>"
```

查看状态：

```sh
sudo -E scripts/connectivity-relay-check.sh status
```

## 6. 查看设备和 Overlay IP

在 Control 主机执行：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 device list
```

记录两台设备的 `IPV4`：

```text
Agent A: <agent-a-ip>
Agent B: <agent-b-ip>
```

## 7. 验证 P2P Direct Path

在 Agent A 和 Agent B 上分别执行：

```sh
sudo -E scripts/connectivity-relay-check.sh status
```

重点查看 JSON 输出里的：

```json
"peers": [
  {
    "path": "direct",
    "endpoint": "...",
    "rtt_ms": ...
  }
]
```

backend message 中重点看：

```text
direct_peers=1
fast_probe_peers=0
direct_paths=<peer>@<ip>:<port>/rtt=<n>ms
```

说明：

- `fast_probe_peers > 0`：正在快速打洞，等待几秒后再看。
- `path=direct`：P2P UDP 已建立。
- `path=unknown` 且 `fast_probe_peers=0`：当前 endpoint 没打通，见排障。
- `path=relay`：走了 relay fallback，不是 direct。

## 8. 验证 Overlay 通信

在 Agent A ping Agent B：

```sh
ping -c 3 <agent-b-ip>
```

在 Agent B ping Agent A：

```sh
ping -c 3 <agent-a-ip>
```

也可以用脚本：

```sh
sudo -E scripts/connectivity-relay-check.sh ping --target <peer-overlay-ip>
```

成功标准：

- ping 有 reply。
- `status --json` 中 peer `path` 是 `direct`。
- `tx_packets` / `rx_packets` 增长。

## 9. 验证 MagicDNS 访问

如果你也想验证类似 Tailscale 的 `hostname.mesh`：

在任意 agent 上查看 DNS 记录：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 dns records
```

安装系统 resolver：

```sh
sudo -E target/debug/tfscale-agent --state-dir "$TFSCALE_STATE_DIR" dns install
```

然后测试：

```sh
ping -c 3 <peer-hostname>.mesh
```

清理 resolver：

```sh
sudo -E target/debug/tfscale-agent --state-dir "$TFSCALE_STATE_DIR" dns uninstall
```

## 10. Relay Fallback 可选验证

如果 direct 不通，或者想验证 relay fallback：

1. 确认 Control 主机启动了 relay：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 relay list
```

2. 在 Agent A/B 的防火墙中阻断对端 UDP 数据面端口，或放到 symmetric NAT 后。

3. 等待 direct probe 失败后查看：

```sh
sudo -E scripts/connectivity-relay-check.sh status
```

预期：

```text
path=relay
```

注意：当前 relay 是 TCP JSON Lines 原型，主要用于验证 fallback 数据面，不是生产级
TLS relay。

## 11. 排障

### control 不健康

```sh
curl -v http://<control-ip>:8080/healthz
```

检查：

- control 是否监听 `0.0.0.0:8080`。
- 云安全组/防火墙是否放行 TCP 8080。
- agent 使用的 `TFSCALE_CONTROL_URL` 是否是 agent 可访问地址。

### public endpoint 不出现

检查 UDP 3478：

```sh
sudo ss -lunp | grep 3478
```

确认 Control 启动时用了：

```sh
TFSCALE_UDP_PROBE_LISTEN=0.0.0.0:3478
```

确认云安全组/防火墙放行 UDP 3478。

### path 一直是 unknown

先看 status：

```sh
sudo -E scripts/connectivity-relay-check.sh status
```

重点：

- `fast_probe_peers` 是否还大于 0。
- peer 是否有 `endpoint`。
- `failures` 是否持续增长。

常见原因：

- 双方 UDP 出站被阻断。
- control UDP probe 端口不可达，导致没有 public endpoint。
- 两端都在 symmetric NAT 后，direct 可能打不通，需要 relay。
- 本地 LAN endpoint 是内网地址，但双方不在同一 LAN，public endpoint 应该优先。

### ping 不通但 path=direct

检查：

```sh
ip addr show tfscale0
ip route show 100.64.0.0/10
sudo -E target/debug/tfscale-agent --state-dir "$TFSCALE_STATE_DIR" status --json
```

常见原因：

- TUN 创建失败。
- 路由没有指向 `tfscale0`。
- ping 目标不是对端 overlay IP。

### agent 启动失败

查看日志：

```sh
cat .tmp/connectivity-relay/logs/tfscale-agent.log
```

常见原因：

- 没有 root / `CAP_NET_ADMIN`。
- `/dev/net/tun` 不存在。
- control URL 不可达。
- login key 用错或已被使用。

## 12. 清理

Agent A：

```sh
sudo -E scripts/connectivity-relay-check.sh cleanup --state-dir /var/lib/tfscale-agent-a
```

Agent B：

```sh
sudo -E scripts/connectivity-relay-check.sh cleanup --state-dir /var/lib/tfscale-agent-b
```

Control：

```sh
scripts/connectivity-relay-check.sh cleanup
```

如果手动安装过 DNS resolver：

```sh
sudo target/debug/tfscale-agent --state-dir /var/lib/tfscale-agent-a dns uninstall || true
sudo target/debug/tfscale-agent --state-dir /var/lib/tfscale-agent-b dns uninstall || true
```

## 13. 测试结果记录模板

请记录这些信息，方便定位：

```text
Control IP:
Agent A public/LAN IP:
Agent B public/LAN IP:
Agent A overlay IP:
Agent B overlay IP:

Agent A status peer path:
Agent B status peer path:
fast_probe_peers:
direct_peers:
relay_peers:
failures:
rtt_ms:

Agent A -> Agent B ping:
Agent B -> Agent A ping:

是否使用 relay:
失败日志:
```

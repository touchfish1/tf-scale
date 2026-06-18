# v0.2 连接穿透与 Relay 设计

## 目标

本阶段目标是让 tf-scale 从“直接 UDP 可达时能互通”升级为“复杂 NAT 后也能自动连接”。
能力目标对齐 Tailscale 的基础连接体验：

1. 优先尝试同 LAN 直连。
2. 自动发现公网 UDP 映射地址。
3. 对 NAT 后设备进行 UDP hole punching。
4. 直连失败时自动走 DERP-like relay。
5. relay 只转发已加密 backend packet，不能解密用户流量。

开发拆分和逐阶段验收见
[v0.2 连接穿透与 Relay 开发拆分](V0_2_CONNECTIVITY_RELAY_BREAKDOWN.md)。

## 范围

包含：

- STUN-like endpoint discovery。
- endpoint heartbeat 上报和过期。
- peer endpoint ranking。
- 双向 UDP probing / hole punching。
- relay service 原型。
- agent 到 relay 的长连接。
- peer map 下发 relay metadata。
- direct-to-relay fallback。
- 连接状态诊断。

不包含：

- 多 relay region 调度的完整生产化。
- 复杂 ACL。
- MagicDNS。
- 用户/组织体系。
- Web admin。

## 总体架构

```text
Agent A              Control Plane              Agent B
  | endpoint report       |                       |
  |---------------------->|                       |
  |                       |<----------------------|
  |                       | endpoint report       |
  |<------ peer map ------|------ peer map ------>|
  |                       |                       |
  |<==== UDP probes / direct encrypted data ====>|
  |                       |                       |
  |---- relay TLS ----> Relay <---- relay TLS ----|
  |<====== encrypted packet relay fallback =====>|
```

## 数据模型扩展

### Endpoint

当前 endpoint 需要扩展：

```json
{
  "kind": "lan | public | ipv6 | relay",
  "address": "203.0.113.10",
  "port": 51820,
  "protocol": "udp | tcp",
  "source": "local | stun | relay",
  "priority": 100,
  "observed_at": "2026-06-18T12:00:00Z",
  "expires_at": "2026-06-18T12:01:00Z"
}
```

### Relay Metadata

control plane 需要维护 relay 列表：

```json
{
  "relay_id": "relay_sfo_1",
  "url": "https://relay.example.com",
  "region": "sfo",
  "healthy": true,
  "load": 0.31
}
```

peer map 需要包含可用 relay metadata，agent 才能在 direct 失败时 fallback。

## STUN-like Endpoint Discovery

新增 lightweight discovery endpoint：

```text
POST /v1/agent/endpoint-probe
```

agent 从本地 UDP socket 向 control plane 或 relay 的 probe endpoint 发包。服务端返回观察到的来源：

```json
{
  "observed_address": "203.0.113.10",
  "observed_port": 49201,
  "protocol": "udp",
  "udp_probe_address": "203.0.113.10",
  "udp_probe_port": 3478
}
```

实现上，HTTP `endpoint-probe` 负责认证 agent 并返回 UDP probe listener 地址；
agent 随后要求 backend 使用已绑定的数据面 UDP socket 发 probe。这样 public
endpoint 的端口来自实际 backend UDP socket，而不是独立 HTTP/TCP 连接。

agent 将以下 endpoint 上报 heartbeat：

- LAN endpoint：本机私网 IP + backend UDP port。
- Public endpoint：probe 观察到的公网 IP/端口。
- IPv6 endpoint：可用时上报。
- Relay endpoint：agent 已连接的 relay ID。

## Hole Punching 流程

当 agent 收到 peer map 后：

1. 对每个 peer 选择候选 endpoint。
2. 同时向 peer 的 public endpoint 和 LAN endpoint 发送 probe frame。
3. 收到对端任意有效加密 frame 后，将该 endpoint 标记为 direct 可用。
4. direct 可用时，真实 overlay packet 走 direct UDP。
5. direct 超时或连续失败时，切到 relay。

probe frame 必须使用现有 backend crypto 保护，至少包含：

- source device ID
- destination device ID
- nonce
- probe timestamp
- endpoint candidate ID

避免明文可伪造的 probe 影响连接状态。

## Endpoint Ranking

初始排序：

1. LAN IPv4 endpoint，同网段优先。
2. IPv6 endpoint。
3. Public UDP endpoint。
4. Relay endpoint。

运行时根据结果调整：

- 最近成功 direct endpoint 优先。
- 连续失败 endpoint 降级。
- relay 作为可用但低优先级路径。

backend status 增加：

```text
peer=<id> path=direct|relay endpoint=<addr> rtt_ms=<n> failures=<n>
```

## Relay Service 原型

新增 crate：

```text
crates/tfscale-relay/
```

职责：

- 接受 agent TLS/WebSocket 或 HTTP/2 长连接。
- agent 连接后注册 `device_id` 和 relay session。
- relay 按 destination device ID 转发 encrypted frame。
- 不解密 packet payload。
- 向 control plane 上报 relay health。

最小协议：

```json
{
  "type": "register",
  "device_id": "dev_x",
  "node_key": "..."
}
```

```json
{
  "type": "frame",
  "source_device_id": "dev_a",
  "destination_device_id": "dev_b",
  "payload": "<encrypted-frame-bytes>"
}
```

## Agent Fallback 策略

每个 peer 维护 path state：

```text
Unknown -> ProbingDirect -> DirectReady
Unknown -> RelayReady
DirectReady -> RelayReady    after direct failures
RelayReady -> ProbingDirect  periodic direct retry
```

要求：

- relay fallback 不阻塞 direct 探测。
- 走 relay 时仍定期尝试 direct。
- direct 恢复后自动切回 direct。
- packet counters 区分 direct 和 relay。

## Control Plane API 扩展

新增：

- `POST /v1/agent/endpoint-probe`
- `GET /v1/relays`
- `POST /v1/relays/heartbeat`

扩展：

- heartbeat endpoint payload 增加 `source`、`priority`、`expires_at`。
- network map 增加 relay metadata。
- device list/status 可显示当前 path：`direct` / `relay` / `unknown`。

## 安全边界

- relay 不持有 backend private key。
- relay 只看 frame envelope 的 source/destination，用于转发。
- 用户 IP packet payload 已由 backend crypto 加密。
- control plane 只存 public credential 和 endpoint metadata。
- probe frame 必须认证，防止伪造 endpoint 成功状态。

## 实施阶段

### Phase A：Endpoint Discovery

- 扩展 endpoint payload。
- agent 上报 LAN + public endpoint。
- control plane 存储 endpoint source、过期时间。
- network map 下发所有 endpoint candidates。

验收：

- 两个 NAT 后 agent 能看到彼此 public endpoint。
- endpoint 过期后不再下发。

### Phase B：UDP Hole Punching

- 增加 probe frame。
- agent 对 peer endpoint 并发打洞。
- backend 记录 direct path state。
- direct endpoint 成功后优先发送 overlay packet。

验收：

- 常见 cone NAT 下两端无需端口转发可以 direct ping。
- symmetric NAT 下 direct 失败但状态可诊断。

### Phase C：Relay Fallback

- 新增 `tfscale-relay`。
- agent 建立 relay 长连接。
- peer map 下发 relay metadata。
- direct 失败后走 relay。

验收：

- 两个 symmetric NAT 后设备可以通过 relay ping。
- relay packet counters 增长。
- direct 恢复后能自动切回 direct。

### Phase D：诊断和文档

- `tfscale-agent status --json` 输出 peer path state。
- `tfscalectl device list` 显示 endpoint/path 摘要。
- 增加 NAT/relay 验证脚本和文档。

验收：

- 用户能看出当前连接是 direct 还是 relay。
- 失败时能看出是 endpoint 缺失、probe 超时还是 relay 不可用。

## 测试策略

单元测试：

- endpoint ranking。
- endpoint expiration。
- probe frame 加解密。
- path state transition。
- relay frame routing。

集成测试：

- loopback 上模拟 direct success。
- mock NAT 行为下验证 direct failure -> relay fallback。
- relay disconnect 后 path 降级。

实机测试：

- 同 LAN direct。
- 不同公网 NAT direct hole punching。
- symmetric NAT relay fallback。
- Linux/macOS 混合测试。

## 发布标准

本阶段完成后，tf-scale 应满足：

- 普通家庭/办公 NAT 后设备可以自动 direct 或 relay 互通。
- 用户无需手工配置端口转发。
- relay 不解密用户流量。
- status 能明确展示当前 peer path。
- 文档能指导用户复现 direct、relay、fallback 三种路径。

# v0.2 连接穿透与 Relay 开发拆分

## 总览

本计划把 [v0.2 连接穿透与 Relay 设计](V0_2_CONNECTIVITY_RELAY_PLAN.md)
拆成 4 个可独立提交和验收的开发阶段：

1. Endpoint Discovery：发现并下发 LAN/Public/Relay endpoint。
2. UDP Hole Punching：对候选 endpoint 进行加密探测和直连路径选择。
3. Relay Fallback：新增 relay 服务，direct 失败时走密文中继。
4. Diagnostics & Validation：状态、CLI、脚本和实机验收。

每个阶段都必须保持 `cargo test --workspace` 通过，并避免破坏当前直接 UDP 可达路径。

## Phase 1：Endpoint Discovery

状态：协议字段、endpoint metadata migration、heartbeat 存储、peer map 过期过滤、
HTTP endpoint-probe discovery API、control UDP probe listener、agent 通过 backend
UDP socket 自动探测并上报 public endpoint 已实现。

### 目标

让 agent 自动发现自己的可用 endpoint，并通过 heartbeat 上报给 control plane。
peer map 下发带 source、priority、过期时间的 endpoint candidates。

### 数据结构

扩展 `tfscale-core::protocol::EndpointPayload`：

```rust
pub struct EndpointPayload {
    pub kind: String,       // lan | public | ipv6 | relay
    pub address: String,
    pub port: u16,
    pub protocol: String,   // udp | tcp
    pub source: Option<String>,     // local | stun | relay
    pub priority: Option<i32>,
    pub expires_at: Option<String>,
}
```

数据库迁移：

- 当前 `endpoints` 已有 `source` 和 `latency_ms`。
- 新增 `priority INTEGER`。
- 新增 `expires_at TEXT`。
- heartbeat 写入 endpoint 时保留 `source`，默认 `agent` 改为 payload source 或 `local`。

### API

新增：

```text
POST /v1/agent/endpoint-probe
```

请求：

```json
{
  "device_id": "dev_x",
  "node_key": "...",
  "protocol": "udp"
}
```

响应：

```json
{
  "observed_address": "203.0.113.10",
  "observed_port": 49201,
  "protocol": "udp",
  "udp_probe_address": "203.0.113.10",
  "udp_probe_port": 3478
}
```

说明：`observed_*` 保留为 HTTP fallback；agent 优先使用 `udp_probe_*`，让
`tfscale-custom` 通过当前 backend UDP socket 发送 probe，避免另开 socket
导致 NAT 映射端口不一致。

### Agent 改动

- `tfscale-agent` 在 heartbeat 前调用 discovery。
- `tfscale-custom::local_endpoints()` 继续报告 LAN UDP endpoint。
- `tfscale-custom::probe_public_endpoint()` 使用 backend 已绑定的 UDP socket
  向 control UDP probe listener 发包并解析观察结果。
- agent 将 LAN endpoint 与 public endpoint 合并上报。
- public endpoint 的 port 必须来自 backend UDP socket 实际端口或 probe 观察端口。

### Control 改动

- heartbeat 存储 endpoint metadata。
- `network_map` 只下发未过期 endpoint。
- `tfscaled serve` 默认监听 `--udp-probe-listen 127.0.0.1:3478`。
- `network_map_version` 应考虑 endpoint 更新时间或版本，而不是只 count。

### 测试

- endpoint payload 向后兼容反序列化。
- heartbeat 写入 source/priority/expires_at。
- expired endpoint 不进入 peer map。
- HTTP endpoint-probe 返回 UDP probe 服务地址。
- UDP endpoint probe 返回服务端观察到的 UDP 来源地址和端口。

### 验收

- 两个 agent 的 peer map 中能看到 LAN endpoint。
- 经过 probe 后能看到 public endpoint。
- endpoint 过期后不再下发。

## Phase 2：UDP Hole Punching

状态：加密 `probe` / `probe_response` frame、endpoint ranking、backend direct path
state、收到 probe 自动回 probe_response、收到 probe_response 后记录 active direct
endpoint 已实现。后台周期性 probe 调度、失败降级和 RTT 统计仍待实现。

### 目标

对 peer endpoint candidates 进行并发探测，NAT 允许时自动建立 direct path。

### Backend 数据结构

新增 peer path state：

```rust
enum PeerPathKind {
    Unknown,
    Direct,
    Relay,
}

struct PeerPathState {
    device_id: String,
    current_path: PeerPathKind,
    active_endpoint: Option<Endpoint>,
    last_probe_at: Option<Instant>,
    last_success_at: Option<Instant>,
    failures: u32,
    rtt_ms: Option<u64>,
}
```

### Frame

新增 frame message type：

- `data`
- `probe`
- `probe_response`

probe/probe_response 仍使用现有 crypto session 加密认证。

当前实现中 probe payload 为轻量控制消息，认证依赖现有 XChaCha20-Poly1305
session、frame source/destination 和 nonce replay window。

### Transport 改动

- 对每个 peer endpoint 启动 probe：已实现 backend 原语，后台调度待接入。
- 收到有效 probe 后回复 probe_response：已实现。
- 收到 probe_response 后记录 active direct endpoint：已实现。
- data packet 优先走 active direct endpoint：已实现为更新 peer session endpoint。
- direct endpoint 连续失败后降级为 unknown，等待 relay 或重试：待实现。

### Endpoint Ranking

排序规则：

1. LAN endpoint。
2. IPv6 endpoint。
3. Public UDP endpoint。
4. Relay endpoint。

同类 endpoint 中：

- 最近成功优先。
- RTT 低优先。
- failures 少优先。

### 测试

- probe frame round-trip：已覆盖。
- invalid probe 被拒绝。
- endpoint ranking：已覆盖。
- direct success 更新 peer path：已覆盖。
- direct failure 触发降级。

### 验收

- 同 LAN 仍走 direct。
- 常见 cone NAT 下 public endpoint probe 成功。
- status 能显示 peer 当前 direct endpoint。

## Phase 3：Relay Fallback

### 目标

新增 DERP-like relay 原型。direct 失败时，agent 自动通过 relay 转发密文 frame。

### 新 crate

```text
crates/tfscale-relay/
```

最小能力：

- `tfscale-relay serve --listen 0.0.0.0:9443`
- agent 建立 WebSocket 或 HTTP/2 长连接。
- agent register relay session。
- relay 根据 destination device ID 转发 encrypted frame。
- relay 不解密 payload。

### Relay 协议

```json
{ "type": "register", "device_id": "dev_a", "node_key": "..." }
```

```json
{
  "type": "frame",
  "source_device_id": "dev_a",
  "destination_device_id": "dev_b",
  "payload": "<base64 encrypted frame>"
}
```

### Control Plane

新增 relay metadata：

- migration 增加 `relays` 表。
- `GET /v1/relays` 返回可用 relay。
- network map 附带 relay candidates。

### Agent/Backend

- agent 从 network map 获取 relay metadata。
- backend 建立 relay transport。
- direct path unavailable 时，data frame 走 relay。
- relay 路径不阻塞 direct probe，后台持续尝试 direct。

### 测试

- relay session register。
- relay frame route 到目标 session。
- unknown destination 返回 drop/error。
- direct failure 后选择 relay。
- direct 恢复后切回 direct。

### 验收

- 两个不能 direct 的 agent 可以通过 relay ping。
- relay counters 增长。
- relay 不需要 backend private key。

## Phase 4：Diagnostics & Validation

### 目标

让用户能明确知道每个 peer 当前走 direct 还是 relay，以及失败原因。

### Agent Status

扩展 `tfscale-agent status --json`：

```json
{
  "peers": [
    {
      "device_id": "dev_b",
      "path": "direct",
      "endpoint": "203.0.113.10:49201",
      "rtt_ms": 18,
      "tx_packets": 12,
      "rx_packets": 11,
      "failures": 0
    }
  ]
}
```

### CLI

可选新增：

```sh
tfscalectl device list --json
tfscalectl relay list
```

### 验证脚本

新增：

```text
scripts/connectivity-relay-check.sh
```

支持：

- `preflight`
- `control`
- `relay`
- `agent`
- `status`
- `ping`
- `cleanup`

### 文档

新增：

- NAT direct 验证文档。
- relay fallback 验证文档。
- 常见失败说明：endpoint 过期、UDP blocked、symmetric NAT、relay unavailable。

### 验收

- 用户能判断 peer path 是 `direct` 还是 `relay`。
- 能看到当前 endpoint、RTT、失败次数和 packet counters。
- direct、relay、fallback 三类路径都有可复现测试步骤。

## 建议提交顺序

1. 扩展 protocol endpoint 字段和兼容转换。
2. 增加 endpoint migration 和 heartbeat/network map 存取。
3. 实现 endpoint probe API。
4. agent 合并 LAN/public endpoints 并上报。
5. backend endpoint ranking。
6. probe/probe_response frame。
7. direct path state 和 probing loop。
8. 新增 `tfscale-relay` crate。
9. relay session 和 frame forwarding。
10. agent/backend relay transport。
11. direct-to-relay fallback。
12. status/CLI/脚本/文档。

## 风险

- 不同 NAT 类型行为差异大，需要实机矩阵验证。
- symmetric NAT 通常无法 direct，需要 relay 兜底。
- relay 长连接协议要避免阻塞 backend packet loop。
- endpoint 过期和 network_map_version 需要稳定，否则 agent 可能错过更新。
- relay metadata 后续需要 TLS、鉴权和 region 选择生产化。

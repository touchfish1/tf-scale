# v0.3 Phase 3 Local DNS Proxy 详细设计

## 背景与问题

Phase 1 和 Phase 2 已经完成 control DNS records、network map 下发和 agent 本地
DNS snapshot 保存，但用户还不能直接解析 `devbox.mesh`。因此下一阶段必须交付一个
可见能力：agent 本地启动 MagicDNS UDP server，能用标准 DNS 工具查到 overlay IP。

## 用户可见目标

本阶段完成后，在 agent 主机上可以执行：

```sh
dig @127.0.0.1 -p 1053 devbox.mesh A
```

预期返回：

```text
devbox.mesh. 30 IN A 100.64.0.x
```

暂不自动修改系统 resolver，因此 `ping devbox.mesh` 不作为本阶段必达目标。系统 DNS
接入留到 Phase 4。

## 范围

包含：

- agent 启动本地 UDP DNS listener。
- 默认监听 `127.0.0.1:1053`，支持 CLI 参数覆盖。
- 从 agent `state.json` 的 `dns_records` 读取 snapshot。
- 支持 `A` 查询，匹配 `*.mesh`。
- 命中返回 A record，未命中返回 NXDOMAIN。
- 非 `A` 查询返回 NOERROR 空答案或 NOTIMP，具体由实现库的最小代价决定。
- `tfscale-agent status --json` 展示 DNS listener 配置和最近 snapshot 数量。
- 增加 Linux 验证文档和脚本入口。

不包含：

- 自动写 `/etc/resolv.conf`。
- systemd-resolved、NetworkManager、macOS `/etc/resolver/mesh` 接入。
- DNS over TCP。
- upstream forwarding。
- 多 suffix 和自定义 search domain。

## CLI 与运行方式

在 `tfscale-agent up` 增加参数：

```sh
tfscale-agent up \
  --login-key <key> \
  --control-url http://127.0.0.1:8080 \
  --dns-listen 127.0.0.1:1053
```

默认行为：

- `--dns-listen` 默认 `127.0.0.1:1053`。
- 如果端口被占用，agent 不应整体退出；DNS 状态记录为 failed，backend 继续运行。
- 后续可增加 `--disable-dns`，本阶段如果实现成本低可以一并加入。

## 模块设计

建议在 agent crate 内新增模块：

```text
crates/tfscale-agent/src/dns.rs
```

核心类型：

```rust
struct DnsConfig {
    listen: SocketAddr,
    suffix: String, // MVP 固定 mesh
}

struct DnsSnapshot {
    records: Vec<DnsRecord>,
}

struct DnsRuntimeStatus {
    enabled: bool,
    listen: SocketAddr,
    healthy: bool,
    records: usize,
    message: Option<String>,
}
```

核心函数：

```rust
spawn_dns_proxy(config, state_dir, stop) -> JoinHandle<()>
resolve_a(records, qname) -> Option<Ipv4Addr>
```

DNS runtime 每次请求时读取内存 snapshot。agent 应在 network map 更新后刷新共享
snapshot，而不是每次 DNS 查询读磁盘。

## 数据流

```text
control dns_records
        |
        v
network map dns_records
        |
        v
agent state + in-memory DNS snapshot
        |
        v
UDP DNS query devbox.mesh A -> 100.64.0.x
```

## 实现选择

优先使用成熟 DNS crate，避免手写 DNS wire format。候选：

- `hickory-server` / `hickory-proto`：功能完整，适合后续扩展。
- `trust-dns-proto`：旧命名生态，需确认当前 crate 维护状态。

建议先使用 `hickory-proto` 手动解析/编码 UDP DNS packet；如果 server trait 成本较低，
再使用 `hickory-server`。本阶段功能很小，重点是可测和可运行。

## 测试计划

单元测试：

- `resolve_a` 命中 `devbox.mesh`。
- 查询大小写不敏感：`DevBox.Mesh`。
- 未命中返回 none/NXDOMAIN。
- 非 mesh 域名不返回 overlay IP。
- trailing dot 兼容：`devbox.mesh.`。

集成/冒烟：

- 启动 DNS proxy，发送 UDP DNS A 查询，断言返回 `100.64.0.x`。
- 使用临时 state 写入 DNS records，不依赖 root。
- `cargo test -p tfscale-agent` 覆盖 DNS 解析和 UDP loop。

手工验证：

```sh
target/debug/tfscale-agent --state-dir ./state status --json
dig @127.0.0.1 -p 1053 devbox.mesh A
dig @127.0.0.1 -p 1053 missing.mesh A
```

## 验收标准

- agent up 后本地 UDP DNS listener 可启动。
- `dig @127.0.0.1 -p 1053 <hostname>.mesh A` 返回 overlay IP。
- rename 后等待一个 poll 周期，新 hostname 可解析，旧 hostname NXDOMAIN。
- delete 后等待一个 poll 周期，该 hostname NXDOMAIN。
- DNS 端口不可用时，agent 数据面不受影响，status 能显示 DNS failed。
- `cargo test --workspace` 通过。

## 开发拆分

1. 引入 DNS 依赖并新增 `dns.rs`。
2. 实现 snapshot 到 A record lookup。
3. 实现 UDP DNS query parse/response encode。
4. 在 agent up 中启动 DNS runtime。
5. network map 更新时刷新 in-memory DNS snapshot。
6. status 增加 DNS runtime 状态。
7. 增加单元测试和 UDP loop 测试。
8. 补充中文验证文档，给出 `dig` 命令。

## 风险与后续

- 1053 非标准端口需要用户显式 `dig @127.0.0.1 -p 1053`，体验还不是最终态。
- 自动接管系统 resolver 涉及 root、发行版差异和 macOS 特殊路径，应在 Phase 4 单独做。
- DNS snapshot 来自 polling，rename/delete 生效有 poll interval 延迟。
- 后续支持 `ping devbox.mesh` 需要接入 systemd-resolved 或 macOS `/etc/resolver/mesh`。

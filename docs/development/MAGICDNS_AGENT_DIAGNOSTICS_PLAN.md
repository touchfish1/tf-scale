# MagicDNS 与 Agent 一体化诊断计划

## 目标

当前 agent 已经能启动本地 MagicDNS UDP proxy，并在 `status --json` 中输出 backend、
DNS records 和 peer path。但用户排查问题时仍要在多个命令之间跳转。本阶段目标是先
提供一个统一的本机诊断体验：

```sh
tfscale-agent doctor
```

输出应能回答：

- agent 是否已注册，overlay IP 是什么。
- backend/TUN/UDP 是否健康。
- MagicDNS listener 是否启动。
- DNS snapshot 是否有记录。
- 系统 resolver 是否已经接入。
- peer 当前走 direct、relay 还是 unknown。
- 下一步该执行什么命令。

## 用户场景

1. 用户执行 `dig @127.0.0.1 -p 1053 devbox.mesh A` 失败。
2. 用户执行 `ping devbox.mesh` 失败。
3. 用户能 ping overlay IP，但不能按 hostname 访问。
4. 用户不确定当前 peer 是 direct 还是 relay。
5. 用户想把诊断输出贴给开发者或 issue。

## 范围

包含：

- `tfscale-agent doctor`。
- `tfscale-agent doctor --json`。
- MagicDNS listener、DNS snapshot、resolver plan/status 的统一诊断。
- backend health 和 peer path 摘要。
- 诊断建议文案。
- MagicDNS 验证脚本调用 doctor。

不包含：

- control plane 全局设备状态聚合。
- Web UI。
- ACL 诊断。
- 自动修复系统配置。

## 诊断模型

新增 agent 本地诊断结构：

```rust
struct AgentDoctorReport {
    overall: DoctorLevel,
    checks: Vec<DoctorCheck>,
}

struct DoctorCheck {
    id: String,
    level: DoctorLevel, // ok | warn | fail
    summary: String,
    detail: Option<String>,
    suggestion: Option<String>,
}
```

建议 check：

- `state.registered`：`device_id`、`node_key`、`ipv4` 是否存在。
- `backend.healthy`：复用 `NetworkBackend::status()`。
- `backend.peers`：peer path direct/relay/unknown 数量。
- `dns.listener`：`state.dns_enabled`、`dns_healthy`、`dns_listen`。
- `dns.snapshot`：`dns_records.len()`。
- `dns.resolver_plan`：当前平台 resolver plan 是否可生成。
- `dns.system_resolver`：后续 Phase 4B 实现 install/status 后再检查实际文件。

## CLI 输出

文本输出示例：

```text
tf-scale doctor

OK   state.registered    dev_... 100.64.0.2
OK   backend.healthy     tfscale0 transport_ready
WARN backend.peers       direct=0 relay=1 unknown=0
OK   dns.listener        127.0.0.1:1053 records=2
WARN dns.system_resolver not installed

Next:
  dig @127.0.0.1 -p 1053 devbox.mesh A
  sudo tfscale-agent dns install
```

JSON 输出用于脚本：

```sh
tfscale-agent doctor --json
```

## 与 MagicDNS 的关系

MagicDNS 排查路径应固定为：

1. `tfscale-agent doctor`
2. `tfscale-agent status --json`
3. `tfscalectl dns records`
4. `dig @127.0.0.1 -p 1053 <name>.mesh A`
5. `tfscale-agent dns plan`
6. 后续 `tfscale-agent dns status`

doctor 不应代替 `status --json`，而是提供面向人的摘要和下一步建议。

## 开发拆分

### Phase 1：Doctor Report

- 新增 doctor 数据结构。
- 从 `AgentState` 和 `BackendStatus` 构建 checks。
- 文本输出和 JSON 输出。
- 单元测试覆盖 overall 级别和核心 check。

### Phase 2：MagicDNS Checks

- 检查 DNS listener 状态。
- 检查 DNS snapshot 是否为空。
- 展示 resolver plan。
- 如果 DNS snapshot 为空，建议检查 control DNS records 和 network map。

### Phase 3：脚本集成

- `scripts/magicdns-local-check.sh doctor`。
- `resolve` 失败时提示运行 doctor。
- 文档更新。

### Phase 4：后续与系统 resolver

- `tfscale-agent dns install|uninstall|status` 落地后，doctor 读取真实 resolver status。
- `ping devbox.mesh` 失败时能区分“本地 DNS server 没问题，但系统 resolver 未接入”。

## 验收

- 未注册 agent：doctor 显示 `state.registered=fail`。
- DNS listener 未启动：doctor 显示 `dns.listener=warn/fail`。
- DNS records 为空：doctor 显示 `dns.snapshot=warn`。
- 有 relay peer：doctor 输出 relay peer 数量。
- `cargo test --workspace` 通过。

## 推荐下一步

优先实现 Phase 1 和 Phase 2。完成后再做 `dns install`，因为 doctor 会成为
`ping devbox.mesh` 失败时的第一诊断入口。

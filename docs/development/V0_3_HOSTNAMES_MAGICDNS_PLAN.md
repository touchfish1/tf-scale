# v0.3 Hostnames 与 MagicDNS 计划

## 目标

本阶段把设备 hostname 从“展示字段”升级为可解析的 MagicDNS 数据源。完成后，
control plane 能生成 `hostname.mesh` 记录，CLI 能查看 DNS records，后续 agent
可以基于同一 API 启动本地 DNS proxy。

## 范围

包含：

- 注册和重命名统一 hostname normalize、校验和唯一性检查。
- 生成 `A` 记录：`<hostname>.mesh -> device ipv4`。
- 设备删除后移除对应 DNS record。
- 新增 DNS records API 和 CLI 查询。
- 为 agent 本地 DNS proxy 预留稳定协议类型。

不包含：

- 本机 DNS proxy 和系统 resolver 配置。
- 自定义 DNS suffix。
- CNAME、AAAA、TXT 等高级记录。
- split DNS 或 upstream forwarding。

## Phase 1：Control DNS Records

状态：已完成。control plane 已新增 `dns_records` migration、`GET /v1/dns/records`、
register/rename/delete 记录维护，CLI 已支持 `tfscalectl dns records`。

数据模型新增 `dns_records`：

```text
id, network_id, device_id, name, type, value, created_at, updated_at
```

规则：

- `name` 使用 FQDN 风格，如 `devbox.mesh`。
- MVP 只生成 `A` 记录。
- register 时创建记录，rename 时更新记录，delete 时删除记录。
- hostname 冲突返回 `409`，非法 hostname 返回 `400`。

API：

```text
GET /v1/dns/records
```

返回：

```json
[
  {
    "device_id": "dev_x",
    "name": "devbox.mesh",
    "type": "A",
    "value": "100.64.0.2"
  }
]
```

CLI：

```sh
tfscalectl dns records
tfscalectl --json dns records
```

## Phase 2：Agent DNS Snapshot

状态：已完成。network map 已携带 `dns_records`，agent 会在同步到新
`network_map_version` 时保存 DNS snapshot 到本地 `state.json`，并在
`tfscale-agent status --json` 中展示。此阶段不修改系统 DNS。

agent 从 control 拉取 DNS records 或从 network map 中接收 DNS snapshot，并保存到
本地 runtime。此阶段只同步数据，不修改系统 DNS。

## Phase 3：Local DNS Proxy

agent 启动本地 UDP DNS listener，只处理 `*.mesh` 查询，非 mesh 查询交给系统
resolver。Linux/macOS 的系统 DNS 配置分别实现，并提供清理逻辑。

## 验收

- 注册设备后能通过 CLI 看到 `hostname.mesh`。
- rename 后旧记录消失，新记录出现。
- delete 后记录消失。
- `cargo test --workspace` 通过。

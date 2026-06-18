# v0.3 Phase 4 System Resolver 详细设计

## 目标

Phase 3 已经支持显式查询：

```sh
dig @127.0.0.1 -p 1053 devbox.mesh A
```

Phase 4 的目标是接入系统 resolver，让用户可以直接：

```sh
ping devbox.mesh
curl http://devbox.mesh:8080
```

## 范围

包含：

- Linux systemd-resolved 接入。
- macOS `/etc/resolver/mesh` 接入。
- agent status 展示 resolver 配置状态。
- down/cleanup 时移除 tf-scale 写入的 resolver 配置。
- 失败时不影响 overlay 数据面和本地显式 DNS 查询。

不包含：

- NetworkManager 专有配置。
- Windows DNS 配置。
- 自定义 DNS suffix。
- 全局 DNS 接管或 upstream forwarding。

## Linux 方案

优先支持 systemd-resolved。使用一个 dummy/link 级配置文件，避免改全局
`/etc/resolv.conf`：

```text
/etc/systemd/resolved.conf.d/tfscale-magicdns.conf
```

内容：

```ini
[Resolve]
DNS=127.0.0.1:1053
Domains=~mesh
```

写入后执行：

```sh
systemctl reload systemd-resolved
resolvectl domain
resolvectl dns
```

说明：

- `~mesh` 表示 route-only domain，只把 `mesh` 后缀交给本地 DNS。
- 不改普通 DNS 域名的解析路径。
- 需要 root 权限。

清理：

```sh
rm -f /etc/systemd/resolved.conf.d/tfscale-magicdns.conf
systemctl reload systemd-resolved
```

## macOS 方案

使用 resolver 专用文件：

```text
/etc/resolver/mesh
```

内容：

```text
nameserver 127.0.0.1
port 1053
```

清理：

```sh
rm -f /etc/resolver/mesh
```

说明：

- macOS 会把 `*.mesh` 查询交给该 resolver。
- 需要 root 权限。

## Agent CLI

建议增加显式命令，先不默认自动写系统配置：

```sh
tfscale-agent dns install
tfscale-agent dns uninstall
tfscale-agent dns status
```

默认参数：

- suffix：`mesh`
- nameserver：从 agent state 的 `dns.listen` 读取，默认 `127.0.0.1:1053`

后续可在 `up` 增加：

```sh
tfscale-agent up --install-dns
```

## 实现拆分

### Phase 4A：Resolver Plan

先实现无副作用的配置规划：

- Linux 生成写入路径、文件内容、reload 命令。
- macOS 生成写入路径、文件内容、清理路径。
- 单元测试覆盖 Linux/macOS 输出。

### Phase 4B：CLI Install/Uninstall

新增 `tfscale-agent dns install|uninstall|status`：

- `install` 写文件并执行 reload。
- `uninstall` 删除文件并执行 reload。
- `status` 检查文件是否存在、内容是否匹配。

### Phase 4C：端到端验证

Linux：

```sh
sudo target/debug/tfscale-agent dns install
dig devbox.mesh A
ping -c 3 devbox.mesh
```

macOS：

```sh
sudo target/debug/tfscale-agent dns install
scutil --dns
ping -c 3 devbox.mesh
```

## 验收标准

- `dns install` 后 `ping devbox.mesh` 可解析 overlay IP。
- `dns uninstall` 后系统 resolver 不再使用 tf-scale 配置。
- 非 `mesh` 域名不受影响。
- resolver 配置失败时 agent 数据面和 `dig @127.0.0.1 -p 1053` 仍可用。
- `cargo test --workspace` 通过。

## 风险

- 不同 Linux 发行版可能没有 systemd-resolved，需给出明确错误。
- `127.0.0.1:1053` 依赖 systemd-resolved 是否支持带端口的 DNS 配置；如果目标系统不支持，
  需要改成 agent 监听 `127.0.0.1:53` 或使用 resolved per-link 配置。
- 写 `/etc` 需要 root，脚本和 CLI 必须清楚提示。

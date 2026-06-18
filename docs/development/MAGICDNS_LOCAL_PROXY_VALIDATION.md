# MagicDNS 本地代理验证指南

本文档验证 v0.3 Phase 3A：agent 内置本地 DNS server。当前阶段不会修改系统
resolver，因此验证命令使用显式 DNS server：

```sh
dig @127.0.0.1 -p 1053 devbox.mesh A
```

## 前置条件

在仓库根目录构建：

```sh
cargo build -p tfscale-agent -p tfscale-control -p tfscalectl
```

或使用脚本自动检查和构建：

```sh
scripts/magicdns-local-check.sh preflight
scripts/magicdns-local-check.sh build
```

准备一台 control 和一台 agent。agent 成功同步 network map 后，`state.json` 中应
包含 `dns_records`。可以先查看：

```sh
target/debug/tfscale-agent --state-dir ./state status --json
```

## 脚本验证流程

在 Linux 主机上执行：

```sh
scripts/magicdns-local-check.sh control
key="$(scripts/magicdns-local-check.sh make-key | tail -n 1)"
sudo scripts/magicdns-local-check.sh agent --login-key "$key"
scripts/magicdns-local-check.sh records
scripts/magicdns-local-check.sh status
```

找到 records 中的 `name` 和 `value` 后验证：

```sh
scripts/magicdns-local-check.sh resolve --name <hostname>.mesh --expect <100.64.0.x>
```

清理：

```sh
sudo scripts/magicdns-local-check.sh cleanup
```

## 启动 Agent DNS Listener

agent `up` 默认监听 `127.0.0.1:1053`：

```sh
target/debug/tfscale-agent --state-dir ./state up \
  --login-key <key> \
  --control-url http://<control-ip>:8080
```

也可以指定端口：

```sh
target/debug/tfscale-agent --state-dir ./state up \
  --login-key <key> \
  --control-url http://<control-ip>:8080 \
  --dns-listen 127.0.0.1:1054
```

如果端口被占用，agent 不会因为 DNS 失败而退出；用 `status --json` 查看
`dns.healthy=false` 和失败原因。

## 验证解析

查询存在的记录：

```sh
dig @127.0.0.1 -p 1053 devbox.mesh A
```

成功标准：

- `ANSWER SECTION` 中出现 `devbox.mesh.`。
- 类型为 `A`。
- 地址是对应设备的 `100.64.0.x` overlay IP。

查询不存在的记录：

```sh
dig @127.0.0.1 -p 1053 missing.mesh A
```

成功标准：

- 返回 `NXDOMAIN`。

## Rename/Delete 验证

在 control host 上执行：

```sh
target/debug/tfscalectl --control-url http://<control-ip>:8080 \
  device rename <device-id> newname
```

等待一个 agent poll 周期后：

```sh
dig @127.0.0.1 -p 1053 newname.mesh A
dig @127.0.0.1 -p 1053 oldname.mesh A
```

成功标准：

- `newname.mesh` 返回 overlay IP。
- `oldname.mesh` 返回 `NXDOMAIN`。

删除设备后同理，等待一个 poll 周期后该 hostname 应返回 `NXDOMAIN`。

## 当前限制

- 还不能直接 `ping devbox.mesh`，因为系统 resolver 尚未接入本地 DNS server。
- 本阶段只支持 UDP DNS 和 `A` record。
- 非 `*.mesh` 查询不会转发到上游 DNS。

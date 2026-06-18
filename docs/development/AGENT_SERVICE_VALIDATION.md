# Agent 服务安装验证指南

本文档验证 `tfscale-agent service` 在 Linux systemd 环境中的安装、状态检查和清理。

## 前置条件

在 Linux 主机上准备 control 和 auth key：

```sh
scripts/magicdns-local-check.sh control
key="$(scripts/magicdns-local-check.sh make-key | tail -n 1)"
```

## 手动验证

先查看 systemd unit 计划：

```sh
sudo target/debug/tfscale-agent service plan \
  --login-key "$key" \
  --control-url http://127.0.0.1:8080
```

安装并启用服务：

```sh
sudo target/debug/tfscale-agent service install \
  --login-key "$key" \
  --control-url http://127.0.0.1:8080
```

重启服务并检查状态：

```sh
sudo systemctl restart tfscale-agent
target/debug/tfscale-agent service status
target/debug/tfscale-agent status --json
```

清理：

```sh
sudo target/debug/tfscale-agent service uninstall
```

## 脚本验证

也可以用 MagicDNS 本地验证脚本跑服务 smoke：

```sh
sudo scripts/magicdns-local-check.sh service-smoke --login-key "$key"
```

该命令会生成 service plan、安装服务、重启 `tfscale-agent`、检查 agent status，然后卸载服务。

## 当前限制

- 当前只实现 Linux systemd。
- macOS launchd 仍在后续阶段。
- `service install` 会把登录 key 写入独立环境文件 `/etc/tfscale/agent.env`，仍然需要 root。

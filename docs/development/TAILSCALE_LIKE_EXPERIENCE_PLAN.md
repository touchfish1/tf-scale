# 接近 Tailscale 体验的方案与开发计划

## 目标体验

tf-scale 的近期目标不是一次性补齐 Tailscale 的全部生产能力，而是先达到一个自托管
MVP 体验：

```sh
tfscaled serve --listen 0.0.0.0:8080 --relay-url tcp://relay.example:9443
tfscale-relay serve --listen 0.0.0.0:9443
tfscalectl auth-key create
sudo tfscale-agent up --login-key <key> --control-url http://control:8080
ping devbox.mesh
ssh devbox.mesh
```

用户不需要手动配置 peer、端口转发、hosts 文件或静态路由。agent 自动注册、拿到
overlay IP、同步 peer map、尝试 direct，失败则 relay，并通过 MagicDNS 解析设备名。

## 当前已经具备

- control plane：auth key、设备注册、device list/rename/delete。
- overlay：Linux/macOS TUN、加密 frame、UDP peer transport。
- NAT/relay：endpoint discovery、UDP probing、direct path state、relay fallback。
- MagicDNS 数据面：DNS records、network map 下发、本地 UDP DNS proxy。
- 诊断：agent status JSON、peer path direct/relay/unknown、relay list、DNS records。
- 验证脚本：Linux TUN、relay fallback、MagicDNS 显式解析。

## 距离目标体验的主要缺口

1. 系统 resolver 尚未接入，`ping devbox.mesh` 还不能直接工作。
2. agent 还不是系统服务，重启后不会自动恢复。
3. control/relay 仍是明文 HTTP/TCP 原型，缺 TLS 和 relay 鉴权。
4. auth key 只有基础一次性能力，缺过期、描述、可复用策略和吊销 CLI。
5. 设备状态还不够像产品：在线/离线、当前路径、DNS、relay、端点摘要分散在多个命令。
6. 缺安装包、systemd unit、macOS launchd plist。
7. 没有 ACL/tag，当前是 full mesh。

## 设计原则

- 先让用户路径顺滑，再补完整生产化。
- 每个阶段都要有可见验收命令。
- 系统配置必须可回滚，`down` 或 `dns uninstall` 不留下脏状态。
- 数据面失败不能拖垮控制面和 DNS 诊断；relay/DNS/resolver 都应独立报错。
- 默认保守：不自动写系统 resolver，直到 `dns install` 成熟。

## 阶段计划

### Phase 1：MagicDNS 系统接入

目标：实现 `ping devbox.mesh`。

开发项：

- `tfscale-agent dns install|uninstall|status`。
- Linux systemd-resolved 配置写入和 reload。
- macOS `/etc/resolver/mesh` 写入和清理。
- `scripts/magicdns-local-check.sh` 增加 `install-dns` 和 `ping-name`。
- status JSON 增加 resolver status。

验收：

```sh
sudo tfscale-agent dns install
ping -c 3 devbox.mesh
sudo tfscale-agent dns uninstall
```

### Phase 2：Agent 系统服务

目标：机器重启后自动加入网络。

开发项：

- `tfscale-agent service install|uninstall|status`。
- Linux systemd unit，加载 state dir、control URL、DNS listen。
- macOS launchd plist。
- 日志路径和诊断命令。

验收：

```sh
sudo tfscale-agent service install --control-url http://control:8080
sudo systemctl restart tfscale-agent
tfscale-agent status --json
```

### Phase 3：一体化状态与诊断

目标：用户一眼知道“为什么连不上”。

开发项：

- `tfscalectl device status`：hostname、IP、last_seen、endpoints、path、relay、DNS。
- `tfscale-agent doctor`：TUN、UDP、control、relay、DNS listener、system resolver。
- MagicDNS、relay、direct path 验证脚本合并为统一 `scripts/doctor.sh`。

验收：

```sh
tfscale-agent doctor
tfscalectl device status <device-id>
```

### Phase 4：安全与生产化传输

目标：控制面和 relay 不再是明文原型。

开发项：

- control plane TLS 配置。
- relay TLS 或 WebSocket/TLS。
- relay register 校验 node key 或 control-issued token。
- auth key 增加 expires_at、description、reusable CLI。
- 最小审计日志：auth key create、device rename/delete、DNS install 不进入 control 审计。

验收：

```sh
tfscaled serve --tls-cert cert.pem --tls-key key.pem
tfscale-relay serve --tls-cert cert.pem --tls-key key.pem
```

### Phase 5：安装与发布体验

目标：用户不用从源码手工跑多个命令。

开发项：

- release binaries。
- Linux install script。
- macOS install script。
- 示例 docker-compose：control + relay。
- 中文快速开始文档。

验收：

```sh
curl -fsSL https://example/install.sh | sh
tfscale-agent up --login-key <key> --control-url https://control.example
```

### Phase 6：策略与组网边界

目标：从 full mesh 走向可控网络。

开发项：

- tags 数据模型和 CLI。
- 简化 ACL policy。
- control 编译 peer visibility。
- status 显示“被 ACL 隐藏”的诊断。

验收：

```sh
tfscalectl acl apply policy.json
tfscalectl device list --tag server
```

## 推荐立即执行顺序

1. Phase 1A：实现 `tfscale-agent dns install|uninstall|status`。
2. Phase 1B：扩展 MagicDNS 脚本，验证 `ping devbox.mesh`。
3. Phase 2A：Linux systemd service install。
4. Phase 3A：`tfscale-agent doctor`。
5. Phase 4A：control TLS。

## 关键风险

- systemd-resolved 是否接受 `DNS=127.0.0.1:1053` 需要 Linux 实机验证；如果不稳定，
  agent 需要支持监听 `127.0.0.1:53` 或使用 per-link resolver 配置。
- macOS resolver 文件支持 `port 1053`，但仍需实机验证。
- relay 当前是 TCP JSON Lines 原型，生产体验前需要 TLS、鉴权和更稳的重连。
- 服务安装会写系统目录，必须保证 uninstall 干净。

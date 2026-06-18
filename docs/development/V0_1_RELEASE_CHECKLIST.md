# v0.1 发布整理清单

## 发布目标

v0.1 的发布目标是交付一个最小可演示的自托管 overlay mesh：

- 单个 SQLite control plane。
- CLI 创建 auth key、列出/重命名/删除设备。
- agent 注册、持久化状态、轮询 heartbeat 和 peer map。
- 自研 userspace backend 配置 TUN/utun。
- 直接可达设备之间通过加密 UDP 数据面通信。
- Linux 与 macOS 作为首批平台。

## 当前状态

已完成：

- control plane、CLI、agent runtime、backend trait。
- SQLite migration、设备注册、IP 分配、device list/rename/delete。
- X25519 backend identity 持久化。
- packet framing、AEAD 加密、nonce/replay 防护。
- Linux TUN 配置、TUN read/write loop、UDP transport loop。
- macOS utun 平台模块和命令规划。
- 本地 control/CLI smoke 脚本。
- Linux/macOS 中文验证文档。

待验收：

- Linux privileged 单机 `single-agent` 验证。
- Linux 双机 overlay ping 验证。
- macOS 实机 build、utun 创建和状态验证。
- Linux/macOS overlay ping parity 验证。
- 删除设备后 peer map 收敛的实机验证。

## 发布前必须完成

1. 在 Linux 主机运行：

```sh
sudo scripts/linux-phase6-udp-tun-check.sh single-agent
```

2. 在两台 Linux 主机验证 overlay ping。
3. 在 macOS 主机运行 `docs/development/MACOS_TUN_VALIDATION.md` 中的流程。
4. 验证 Linux/macOS 双向 overlay ping。
5. 删除一个设备，确认 `device list` 和后续 peer map 不再包含它。
6. 更新 README 的“已验证平台”和“已知限制”。

## 发布验证命令

本地无特权验证：

```sh
cargo fmt --all
cargo test --workspace
cargo check --workspace
scripts/e2e-control-cli-smoke.sh
```

Linux target check：

```sh
cargo check -p tfscale-custom --target x86_64-unknown-linux-gnu
```

macOS target check，优先在 macOS 实机执行：

```sh
cargo check -p tfscale-custom --target x86_64-apple-darwin
cargo check -p tfscale-custom --target aarch64-apple-darwin
```

## 已知限制

- 当前仅支持直接 UDP 可达的 peer，不支持 NAT traversal 和 relay fallback。
- control plane 仍是单组织、单网络。
- 没有 MagicDNS、ACL、subnet router、exit node。
- TUN/utun 验证需要 root 或平台网络管理权限。
- macOS target check 在当前 Windows 开发机上因 Apple target 下载失败未完成。
- Windows agent 不在 v0.1 范围内。

## 发布判定

满足以下条件后可以标记 v0.1 ready：

- 所有本地测试和 check 通过。
- Linux 单机和双机验证通过。
- macOS 实机 utun 验证通过。
- Linux/macOS overlay ping 通过。
- 删除设备后的 peer map 收敛验证通过。
- README 与验证文档包含最新命令和限制。


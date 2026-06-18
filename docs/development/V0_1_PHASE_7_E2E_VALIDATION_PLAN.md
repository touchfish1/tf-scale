# v0.1 Phase 7 End-to-End Validation Plan

## 目标

Phase 7 的目标是把 v0.1 从“功能已实现”推进到“开发者和评审者可复现验收”。
本阶段不引入新的数据面能力，重点是脚本、检查清单、故障诊断和验收证据。

## 当前阶段位置

Phase 6 已完成 UDP/TUN runtime 的开发，并补充了 Linux 验证脚本与中文文档。
Phase 7 接在 Phase 6 之后，用真实或准真实环境验证完整链路：

1. control plane 启动。
2. CLI 创建 auth key。
3. 两个 agent 注册并获得不同 overlay IP。
4. agent 获取 peer map 并配置 `tfscale0`。
5. 两个直接可达设备通过 overlay IP ping 通。
6. 删除设备后，后续 peer map 不再包含该设备。

## 范围

### 包含

- 整理端到端验收脚本，覆盖 control、CLI、agent、status、cleanup。
- 补齐双机 Linux 验收流程，记录预期输出和失败排查。
- 验证设备删除后的 peer map 收敛。
- 增加无需 root 的控制面/CLI 冒烟测试。
- 明确 Linux privileged 验证所需权限。

### 不包含

- NAT traversal、relay、endpoint ranking。
- 静态配置加密。
- macOS TUN 实机适配开发。
- CI 中默认运行需要 root/TUN 的测试。

## 产物

- `scripts/e2e-control-cli-smoke.sh`：无需 root 的本地控制面和 CLI 冒烟脚本。
- `scripts/linux-phase6-udp-tun-check.sh`：继续作为 Linux TUN/UDP 实机验证入口。
- `docs/development/E2E_VALIDATION.md`：中文端到端验收手册。
- `docs/development/LINUX_TUN_VALIDATION.md`：保留 Linux 专项验证细节，并从 E2E 文档链接。

## 验收流程

### 1. 本地控制面和 CLI 冒烟

脚本启动临时 `tfscaled`，创建 auth key，检查 `/healthz`，并验证
`tfscalectl device list` 可以访问 control plane。

成功标准：

- control plane 健康检查返回成功。
- auth key 创建成功，输出以 `tfk_` 开头。
- device list 返回空列表或已有设备列表，不报错。

### 2. 单机 Linux agent 冒烟

运行：

```sh
sudo scripts/linux-phase6-udp-tun-check.sh single-agent
```

成功标准：

- `tfscale0` 存在。
- `100.64.0.0/10` 路由指向 `tfscale0`。
- agent status 包含 `tun_configured=true`、`udp_bound=true`、
  `transport_running=true`。

### 3. 双机 overlay ping

一台 control host 暴露 `0.0.0.0:8080`，两台 agent host 各使用一个 auth key
注册。注册后通过 `tfscalectl device list` 获取两个 overlay IP，并互相 ping。

成功标准：

- 两台设备获得不同的 `100.64.0.x`。
- 两台设备都能看到对端 peer。
- `ping -c 3 <peer-overlay-ip>` 成功。
- ping 后 `tx_packets` 和 `rx_packets` 均非 0。

### 4. 设备删除和 peer map 收敛

删除其中一个设备：

```sh
target/debug/tfscalectl device delete <device-id>
```

等待一个 agent poll 周期后，在存活 agent 上检查状态和日志。

成功标准：

- `tfscalectl device list` 不再显示被删除设备。
- 存活 agent 后续 peer map 不再包含被删除设备。
- 对被删除设备 overlay IP 的 ping 不应继续成功。

## 故障排查清单

- `/dev/net/tun` 不存在：加载 `tun` 模块，容器中传入 `--device /dev/net/tun`。
- `Operation not permitted`：使用 root，或授予 `CAP_NET_ADMIN`。
- `tfscale0` 已存在：先执行 `sudo scripts/linux-phase6-udp-tun-check.sh cleanup`。
- 控制机不可达：检查防火墙、监听地址和 `TFSCALE_CONTROL_URL`。
- UDP 不通：检查两台 agent 间的防火墙和安全组。
- peer map 陈旧：等待一个 poll 周期，或重启 agent 重新拉取。
- packet counters 不增长：确认 ping 的目标是对端 overlay IP，而不是本机 IP。

## 实施步骤

1. 新增本地 control/CLI 冒烟脚本。
2. 新增中文 E2E 验收文档。
3. 在 Linux 验证文档中链接 E2E 主流程。
4. 补充脚本输出中的成功提示，便于复制验收结果。
5. 运行 `cargo test --workspace` 和脚本语法检查。
6. 在 Linux 实机执行单机和双机验证，记录结果。

## 后续阶段

按当前 v0.1 路线，Phase 7 是 v0.1 Linux 端到端验收阶段。之后还剩：

1. macOS TUN/utun 支持与实机验证。
2. v0.1 发布整理，包括 README 快速开始、已知限制和 release checklist。


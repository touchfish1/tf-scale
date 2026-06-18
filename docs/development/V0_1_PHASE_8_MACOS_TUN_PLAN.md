# v0.1 Phase 8 macOS TUN/utun Plan

## 目标

Phase 8 补齐 v0.1 的 macOS 平台支持，使 macOS agent 能创建 utun 接口、配置
overlay IP、安装 overlay 路由，并复用 Phase 6 已完成的 UDP/TUN packet loop。

本阶段完成后，v0.1 的平台验收目标是：一台 Linux agent 和一台 macOS agent 在
直接可达网络中注册到同一个 control plane，并通过 overlay IP 互相 ping 通。

## 当前基线

已完成：

- Linux TUN 创建、地址配置、路由配置和 cleanup。
- TUN read/write 边界已经封装在 `tfscale-custom::tun`。
- UDP transport、packet crypto、runtime loops 已实现。
- E2E 验收脚本和 Linux 验证文档已准备好。

本阶段实现中：

- `platform::macos` 模块。
- `PlatformTunDevice` 的 macOS variant。
- macOS 地址、路由和 cleanup 命令规划。
- macOS 实机验证文档。

仍缺：

- macOS 实机构建和 privileged utun 验证结果。

## 设计决策

### utun 接口命名

macOS utun 接口通常由系统分配，例如 `utun4`。agent 的期望接口名仍可保持
`tfscale0` 作为逻辑名，但 backend status 必须报告真实接口名。

实现要求：

- 创建 utun 后读取实际接口名。
- `TunStatus.interface_name` 使用实际 utun 名。
- route cleanup 使用实际 utun 名，不使用逻辑名。

### TUN crate

继续使用 `tun-rs`，并把依赖扩展到 macOS：

```toml
[target.'cfg(any(target_os = "linux", target_os = "macos"))'.dependencies]
tun-rs = "2.8.5"
```

如果 `tun-rs` 的 macOS builder API 与 Linux 命名能力不同，优先接受系统分配的
utun 名，再通过命令配置地址和路由。

## macOS 配置流程

`apply_local_config()` 收到 `LocalBackendConfig` 后：

1. 创建 utun 设备。
2. 获取实际接口名，例如 `utun4`。
3. 设置接口地址：

```sh
ifconfig <utun> inet <overlay-ip> <overlay-ip> netmask 255.255.255.255 up
```

4. 安装 overlay 路由：

```sh
route -n add -net 100.64.0.0/10 -interface <utun>
```

5. 将 utun device handle 存入 runtime，供 read/write loops 使用。
6. backend status 报告 `tun_configured=true` 和实际接口名。

## Cleanup 流程

`tfscale-agent down` 或 agent 正常退出时：

```sh
route -n delete -net 100.64.0.0/10 -interface <utun>
ifconfig <utun> down
```

utun 设备通常会在 fd 关闭后消失，因此 cleanup 不依赖删除接口命令。

## 错误处理

需要给出明确错误：

- 没有 root 权限或缺少网络管理权限。
- `ifconfig` 不存在。
- `route` 不存在。
- utun 创建失败。
- 地址配置失败。
- overlay 路由冲突。

错误信息应进入 backend status 的 `message`，并可通过：

```sh
tfscale-agent status --json
```

查看。

## 测试策略

无需 macOS 权限的单元测试：

- macOS `ifconfig` 命令规划。
- macOS route add/delete 命令规划。
- cleanup 命令使用实际 utun 名。
- 非 macOS/Linux 平台仍返回 unsupported。

本地交叉检查：

```sh
cargo test --workspace
cargo check -p tfscale-custom --target x86_64-apple-darwin
cargo check -p tfscale-custom --target aarch64-apple-darwin
```

如果本机没有 Apple target toolchain，则记录为未执行，交给 macOS 实机验证。
当前 Windows 开发机下载 Apple targets 时网络中断，因此 Apple target check 未执行。

## macOS 实机验收

macOS host：

```sh
cargo build --workspace
sudo TFSCALE_CONTROL_URL=http://<control-host-ip>:8080 \
  target/debug/tfscale-agent --state-dir ./state up --login-key <key>
```

验证：

```sh
target/debug/tfscale-agent --state-dir ./state status --json
ifconfig <utun>
netstat -rn | grep 100.64
ping -c 3 <linux-overlay-ip>
```

成功标准：

- `status --json` 中 backend healthy。
- backend message 包含 `tun_configured=true`、`udp_bound=true`、
  `transport_running=true`。
- macOS 上存在实际 utun 接口。
- `100.64.0.0/10` 路由指向该 utun。
- Linux 和 macOS 能通过 overlay IP 互相 ping。

详细步骤见 [macOS TUN/utun 验证指南](MACOS_TUN_VALIDATION.md)。

## 实施步骤

1. 把 `tun-rs` 依赖扩展到 macOS。
2. 新增 `crates/tfscale-custom/src/platform/macos.rs`。
3. 扩展 `PlatformTunDevice`，支持 Linux 和 macOS 两种内部设备。
4. 在 `platform::mod` 中按 `target_os = "macos"` 分发。
5. 实现 macOS 命令规划和单元测试。
6. 实现 utun 创建、非阻塞 read/write、status、shutdown。
7. 新增 macOS 验证文档或脚本。
8. 运行 workspace 测试和可用的 macOS target check。

## 风险和后续

- Windows 上无法直接验证 Apple target 编译和 macOS 权限行为。
- macOS utun 接口名由系统分配，文档和状态输出必须避免假设固定名称。
- route 命令可能因已有路由返回失败；后续可实现 replace-like 行为。
- v0.2 以后可考虑更细的 endpoint ranking 和 platform diagnostics。

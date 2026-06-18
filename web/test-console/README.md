# tf-scale 测试控制台

这是一个零依赖静态页面，用于辅助 Windows、macOS、Linux 上的测试记录和命令生成。

打开方式：

```text
web/test-console/index.html
```

当前能力：

- 生成 Control、Agent A、Agent B、Status、Ping、MagicDNS 命令。
- 记录两个 agent 的 key、overlay IP、hostname。
- 粘贴 `tfscale-agent status --json` 后解析 direct/relay/unknown 状态。
- 显示 Linux/macOS/Windows 当前测试支持状态。

注意：

- Linux 是当前完整 P2P/overlay ping 主测试平台。
- macOS 和 Windows 当前主要用于 GUI 测试记录；客户端数据面能力还需要继续实现或实机验证。

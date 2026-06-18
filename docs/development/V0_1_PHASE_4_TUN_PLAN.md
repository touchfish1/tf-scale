# v0.1 Phase 4 TUN Adapter Plan

## Goal

Phase 4 makes `tfscale-custom` able to create and configure a local virtual
network interface. Linux is implemented first; macOS follows after the Linux
packet path is proven.

The phase does not implement packet encryption or UDP transport. It prepares a
TUN device and exposes read/write boundaries for later framing and transport
work.

## Current Status

Implemented in the current development branch:

- `tun-rs` is added as a Linux-only dependency.
- `tfscale-custom` has backend-private TUN config/status types.
- Linux command planning is implemented and unit-tested.
- `apply_local_config()` persists local config, attempts TUN setup, and stores
  runtime TUN status.
- Runtime state keeps the configured TUN device handle so later packet loops can
  read and write packets without reopening the interface.
- TUN read/write boundaries are exposed through the backend-private adapter.
- Shutdown takes the runtime TUN device and runs planned Linux cleanup commands.
- Non-Linux platforms return an explicit unsupported-platform error for real
  TUN setup.
- Tests avoid privileged TUN access and cover config/status behavior.
- `cargo check -p tfscale-custom --target x86_64-unknown-linux-gnu` passes.
- Linux manual validation instructions are documented in
  [Linux TUN Validation](LINUX_TUN_VALIDATION.md).

Still remaining:

- Manual privileged Linux validation.
- Packet read/write loop.
- macOS utun implementation.
- Packet framing, crypto, and peer transport.

## Decision

Use `tun-rs` as the TUN integration crate.

Reasons:

- Apache-2.0 license fits this repository.
- Linux TUN and macOS utun are both supported by the same crate family.
- Async Tokio support is available for later packet loops.
- It keeps `/dev/net/tun` and utun platform details out of `tfscale-agent`.

Linux route and address configuration should initially use system commands
behind a small platform module:

- `ip addr add <overlay-ip>/32 dev <interface>`
- `ip link set <interface> up`
- `ip route replace 100.64.0.0/10 dev <interface>`

Direct netlink can replace command execution later if needed.

## Scope

In scope:

- Add a platform TUN module inside `tfscale-custom`.
- Create or open the configured interface name, default `tfscale0`.
- Assign the local `/32` overlay IP on Linux.
- Add the overlay route on Linux.
- Store TUN readiness in backend runtime/status state.
- Return clear errors for missing permissions or missing host tooling.
- Add unit tests around command planning and platform-independent config logic.

Out of scope:

- Packet encryption.
- UDP peer transport.
- NAT traversal.
- Relay fallback.
- macOS implementation in the first Linux pass.
- Windows support.

## Architecture

Keep platform networking private to `tfscale-custom`:

```text
tfscale-agent
  -> NetworkBackend::apply_local_config()
  -> tfscale-custom
     -> state persistence
     -> platform::linux::TunAdapter
     -> tun-rs device
     -> ip address and route commands
```

Suggested modules:

```text
crates/tfscale-custom/src/
  lib.rs
  tun.rs
  platform/
    mod.rs
    linux.rs
    macos.rs
```

`tun.rs` defines backend-private types:

- `TunConfig`
- `TunStatus`
- `TunDevice`
- `PlatformTunDevice`

`platform/linux.rs` owns Linux-specific command construction, command execution,
TUN device creation, packet read/write calls, and cleanup planning.

## Apply Local Config Flow

When `apply_local_config()` receives `LocalBackendConfig`:

1. Persist local config in `custom-backend.json`.
2. Build a `TunConfig` from interface name, overlay IP, and listen port.
3. On Linux, create/open the TUN device.
4. Assign `<overlay_ip>/32`.
5. Bring the interface up.
6. Install or replace route `100.64.0.0/10`.
7. Update runtime status with TUN readiness.
8. Keep the TUN device handle in runtime state until backend shutdown.

If TUN setup fails, preserve the local config in state but report status as
unhealthy with the failure message.

## Permissions and Host Requirements

Linux requirements:

- Access to `/dev/net/tun`.
- `CAP_NET_ADMIN` or root privileges.
- `ip` command available from `iproute2`.
- No conflicting interface or route owned by another program.

Recommended dev invocation:

```sh
sudo target/debug/tfscale-agent --state-dir ./state up --login-key <key>
```

Container requirements, if used later:

- `--cap-add NET_ADMIN`
- `--device /dev/net/tun`

## Error Handling

Return actionable messages for:

- `/dev/net/tun` missing.
- permission denied while creating TUN.
- `ip` command missing.
- address assignment failure.
- route replacement failure.
- unsupported platform.

Examples:

- `missing Linux TUN device: /dev/net/tun`
- `TUN setup requires CAP_NET_ADMIN or root`
- `required command is missing: ip`

## Testing Strategy

Unit tests:

- Convert `LocalBackendConfig` into `TunConfig`.
- Build Linux `ip` command arguments.
- Reject unsupported platforms with `BackendError::UnsupportedPlatform`.
- Preserve state when TUN setup is skipped or mocked.
- Report both `tun_configured` and `tun_io_ready` in backend status.
- Shutdown with no active TUN device is a successful no-op.

Integration/manual tests:

- Linux host can create `tfscale0`.
- `ip addr show tfscale0` shows assigned `/32`.
- `ip route show 100.64.0.0/10` points to `tfscale0`.
- Running `tfscale-agent down` releases runtime resources and removes the route
  and link when the backend owns the TUN device.

Use `scripts/linux-tun-check.sh` for Linux preflight checks and command
guidance.

CI should avoid requiring privileged TUN access at first. Privileged tests can be
added later behind an explicit feature or script.

## Acceptance Criteria

- `cargo test --workspace` passes without host network privileges.
- Linux build compiles with the TUN adapter enabled.
- `tfscale-agent up` attempts TUN setup after receiving an overlay IP.
- Permission/tooling failures are clear and do not corrupt backend state.
- Backend status reports whether TUN is configured.
- A manual Linux run can create/configure `tfscale0` with `100.64.0.x/32`.

## Follow-Up Phases

Phase 5 will connect packet framing and authenticated encryption to TUN packet
reads/writes.

Phase 6 will connect encrypted frames to UDP peer transport.

macOS TUN support should be added after Linux packet flow is validated, using
the same `TunAdapter` boundary where possible.

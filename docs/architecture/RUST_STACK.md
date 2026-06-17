# Rust Stack

tf-scale uses Rust for the control plane, agent, relay service, and CLI.
The encrypted data plane sits behind a pluggable network backend. The first
backend is a self-developed userspace backend, while Rust coordinates identity,
routing, DNS, NAT traversal, relay fallback, and management APIs.

## Workspace Layout

```text
tf-scale/
  crates/
    tfscale-control/     # control plane service
    tfscale-agent/       # node agent daemon
    tfscale-relay/       # DERP-like encrypted packet relay
    tfscalectl/          # operator CLI
    tfscale-core/        # shared domain types, config, errors
    tfscale-acl/         # ACL parser and policy compiler
    tfscale-dns/         # MagicDNS resolver logic
    tfscale-ipam/        # overlay IP allocation
    tfscale-nat/         # endpoint discovery and NAT probing
    tfscale-route/       # OS route and DNS configuration
    tfscale-net/         # network backend traits and shared models
    tfscale-custom/      # self-developed userspace backend implementation
  proto/
  web/
  deploy/
  docs/
```

## Core Crates

| Area | Crates | Notes |
| --- | --- | --- |
| Async runtime | `tokio` | Shared runtime for services, agent, relay, and CLI operations |
| HTTP API | `axum` | Admin API and lightweight service endpoints |
| gRPC | `tonic` | Optional after MVP for streaming network map updates |
| Database | `sqlx` | SQLite for MVP, PostgreSQL for production |
| CLI | `clap` | `tfscalectl` and agent commands |
| Serialization | `serde`, `serde_json` | Config, API payloads, policy documents |
| Errors | `thiserror`, `anyhow` | Library errors and application boundaries |
| Logging | `tracing`, `tracing-subscriber` | Structured logs and diagnostics |
| TLS | `rustls` | Control and relay transport security |
| HTTP client | `reqwest` | Agent and CLI calls to control plane APIs |
| WebSocket | `tokio-tungstenite` | Optional streaming transport if needed |
| TUN | `tun` or `tun-rs` | Platform-specific virtual interface support |

## Custom Backend Strategy

v0.1 implements a minimal tf-scale userspace backend instead of depending on
WireGuard system tooling.

Preferred integration order:

1. Define backend-neutral traits in `tfscale-net`.
2. Implement `tfscale-custom` as the default v0.1 backend.
3. Use userspace TUN on Linux and macOS.
4. Keep packet framing, encryption, and session management inside
   `tfscale-custom`.
5. Add WireGuard or EasyTier later as optional backend crates if useful.

The control plane stores backend public credentials only. Private credentials
are generated and persisted locally by the agent.

## Backend Abstraction

`tfscale-net` owns backend-neutral traits and types. Backends such as
`tfscale-custom`, `tfscale-wg`, or `tfscale-easytier` implement those traits.

The shared interface should cover:

- Backend initialization and shutdown.
- Credential creation and rotation.
- Local address and route application.
- Peer, endpoint, relay, and ACL application.
- Health and diagnostics reporting.
- Capability declaration.

## Service Choices

### Control Plane

- `axum` for REST admin APIs.
- `axum` for v0.1 agent registration, heartbeat, and network map polling.
- `tonic` later for agent-facing gRPC streaming.
- `sqlx` for persistence.
- `tracing` for structured observability.
- SQLite first, PostgreSQL once multi-tenant production requirements appear.

### Agent

- `tokio` daemon runtime.
- `clap` for foreground/debug commands.
- Platform modules for backend, route, and DNS configuration.
- HTTP polling for network map updates in v0.1.
- Local DNS proxy for `*.mesh` queries after v0.1.

### Relay

- `tokio` TCP/TLS server.
- Packet framing for encrypted backend payloads.
- Health and latency reporting back to the control plane.
- No access to decrypted user traffic.

### CLI

- `clap` command tree.
- Human-readable output by default.
- JSON output flag for scripts and tests.

## Open Questions

- When to replace HTTP polling with gRPC streaming.
- Which encryption and packet framing libraries to use inside `tfscale-custom`.
- Which backend capability flags are required for MVP and which can be optional.
- Whether relay packet transport should be custom framed TCP/TLS first or
  WebSocket-compatible for easier deployment through proxies.

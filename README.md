# tf-scale

tf-scale is a self-hosted mesh networking system inspired by Tailscale.
It uses a pluggable network backend for encrypted overlay connectivity.
WireGuard is the first backend for the MVP, while the control plane keeps
device registration, IP allocation, hostname management, peer discovery, ACLs,
NAT traversal, and relay fallback backend-agnostic so EasyTier or a custom
backend can be added later.

## Goals

- Let devices in different LANs access each other through stable virtual IPs.
- Support custom hostnames and MagicDNS-style name resolution.
- Automatically allocate addresses from an overlay network.
- Prefer peer-to-peer encrypted backend connections whenever possible.
- Fall back to DERP-like encrypted relay when direct connectivity fails.
- Keep the control plane unable to decrypt user traffic.
- Keep WireGuard-specific details behind a backend boundary so future
  EasyTier or custom backends do not require a control plane rewrite.
- Provide a clear path from a small self-hosted MVP to a managed multi-tenant
  product.

## Recommended Architecture

```text
                 +----------------------+
                 | Web Admin / CLI      |
                 +----------+-----------+
                            |
                            v
+-------------------+  +----------------------+  +-------------------+
| Node Agent        |  | Control Plane         |  | Relay Service     |
| - Network backend |<-| - Auth / Device       |->| - DERP-like relay  |
| - TUN / Route     |  | - IPAM / DNS          |  | - Region health    |
| - NAT Probe       |  | - Peer Map            |  | - Relay fallback   |
| - Local DNS       |  | - ACL / Route Policy  |  +-------------------+
+---------+---------+  +----------------------+
          |
          | encrypted peer-to-peer first
          v
+-------------------+
| Other Node Agent  |
+-------------------+
```

## Technology Stack

| Layer | Choice | Notes |
| --- | --- | --- |
| Network backend | Pluggable, WireGuard first | End-to-end encrypted node traffic |
| Agent | Rust | System-level networking, memory safety, and long-running daemon reliability |
| Control plane | Rust | Shared protocol models with the agent and relay |
| API | gRPC plus HTTP REST | Streaming agent updates and admin APIs |
| Database | SQLite for MVP, PostgreSQL for production | Start simple, scale later |
| State cache | Redis optional | Online status, heartbeats, relay health |
| Frontend | React plus Vite | Admin console for devices, ACLs, routes |
| Overlay CIDR | `100.64.0.0/10` | CGNAT range suitable for private overlay IPs |
| DNS | Built-in MagicDNS | `hostname.mesh` and custom suffix support |
| Relay | DERP-like relay | Encrypted packet relay when P2P fails |
| Deployment | Docker Compose first | Kubernetes can follow after MVP |

## Documentation

- [Technical Plan](docs/TECHNICAL_PLAN.md)
- [MVP Scope](docs/product/MVP.md)
- [Architecture Decision Records](docs/architecture/ADR.md)
- [Rust Stack](docs/architecture/RUST_STACK.md)
- [Data Model](docs/architecture/DATA_MODEL.md)
- [Network Flows](docs/architecture/NETWORK_FLOWS.md)
- [Development Roadmap](docs/development/ROADMAP.md)

## Initial Repository Layout

```text
tf-scale/
  crates/
    tfscale-control/     # control plane service
    tfscale-agent/       # node agent
    tfscale-relay/       # DERP-like relay service
    tfscalectl/          # CLI
    tfscale-core/        # shared types, config, errors, protocol models
    tfscale-acl/         # policy engine
    tfscale-dns/         # MagicDNS
    tfscale-ipam/        # overlay IP allocation
    tfscale-nat/         # STUN and NAT probing
    tfscale-route/       # OS routing integration
    tfscale-net/         # network backend abstraction
    tfscale-wg/          # WireGuard backend implementation
  proto/                 # gRPC contracts
  web/                   # admin console
  deploy/                # deployment manifests
  docs/                  # design documentation
```

## MVP Target

The first useful milestone is a minimal two-device mesh:

1. A device can register with the control plane.
2. The control plane allocates a stable virtual IP.
3. The agent configures the selected network backend locally.
4. The control plane distributes a peer map.
5. Two devices can ping each other over the overlay network.
6. Devices can be addressed by custom hostnames through MagicDNS.

See [MVP Scope](docs/product/MVP.md) for details.

## References

- [WireGuard Protocol](https://www.wireguard.com/protocol/)
- [EasyTier](https://github.com/EasyTier/EasyTier)
- [Tailscale Control and Data Planes](https://tailscale.com/docs/concepts/control-data-planes)
- [Tailscale DERP](https://tailscale.com/docs/reference/derp-servers)
- [Headscale](https://headscale.net/)

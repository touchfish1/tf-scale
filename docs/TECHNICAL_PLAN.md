# tf-scale Technical Plan

## Positioning

tf-scale follows the Tailscale-style architecture:

- The network backend is a pluggable data-plane boundary.
- WireGuard is the first backend for the MVP.
- The tf-scale control plane coordinates identity, devices, IPAM, hostnames,
  peer discovery, routes, ACLs, and relay selection.
- Node traffic is end-to-end encrypted by the selected backend.
- Relay services forward encrypted packets only and do not decrypt user traffic.

The MVP should not implement a custom VPN encryption protocol. The custom work
belongs in coordination, product workflow, policy, operational tooling, and a
stable backend interface. Future EasyTier or self-developed backends should
plug into that interface without changing the control plane domain model.

## Network Backend Boundary

The agent must depend on a `NetworkBackend`-style interface instead of calling
WireGuard-specific code directly. The first implementation can be WireGuard,
but core concepts should remain backend-neutral:

- `Node`
- `VirtualNetwork`
- `Peer`
- `Route`
- `Credential`
- `Endpoint`
- `TransportConfig`

Suggested backend responsibilities:

- Initialize or tear down the local overlay interface.
- Generate, import, or rotate backend credentials.
- Apply the device's assigned overlay addresses.
- Apply peer, route, endpoint, and relay updates from the network map.
- Report backend health, active sessions, and connectivity diagnostics.
- Declare capabilities such as relay support, NAT traversal, kernel TUN,
  userspace TUN, dynamic peer discovery, and endpoint ranking.

WireGuard-specific fields such as `allowed_ips`, `persistent_keepalive`, and
interface names like `wg0` must stay inside the WireGuard backend. EasyTier or
custom backend fields should follow the same rule.

## Core Components

### Control Plane

Responsibilities:

- User, organization, and network management.
- Device registration and approval.
- Backend public credential storage.
- Overlay IP allocation.
- Hostname uniqueness and DNS record generation.
- Peer map generation.
- ACL compilation.
- Subnet route approval and distribution.
- Relay region health and selection.
- Device state, heartbeats, and audit logs.

The control plane must not store backend private credentials.

### Node Agent

Responsibilities:

- Generate and persist backend credentials.
- Authenticate and register with the control plane.
- Create and configure the selected network backend.
- Assign virtual IPs.
- Configure peer endpoints.
- Configure system routes.
- Run a local DNS resolver or DNS proxy.
- Probe local, public, and relay connectivity.
- Maintain a streaming control connection for peer map updates.
- Apply key rotation and device revocation events.

For the WireGuard backend, Linux should prefer kernel WireGuard when available.
macOS and Windows can use userspace WireGuard implementations where needed.
Alternative backends must stay behind the same agent-facing interface.

### Relay Service

Responsibilities:

- Accept TLS connections from agents.
- Relay encrypted backend packets when direct P2P is unavailable.
- Report health, load, and latency metrics to the control plane.
- Support regional deployment.

The relay service is a fallback path. It should never become the default path
when direct peer connectivity is available.

### Admin Console and CLI

Responsibilities:

- Create auth keys.
- Register and approve devices.
- Rename devices and manage hostnames.
- View online status and endpoints.
- Manage ACLs and tags.
- Approve subnet routes.
- Inspect audit logs.

The CLI should exist before the web console so that MVP development can move
quickly without depending on frontend work.

## API Shape

Use both gRPC and HTTP:

- gRPC for agent registration, heartbeat, and streaming network map updates.
- HTTP REST for admin APIs and browser-based management.

Suggested service groups:

- `AuthService`
- `DeviceService`
- `NetworkMapService`
- `RouteService`
- `DnsService`
- `AclService`
- `RelayService`

## Overlay Addressing

Default network:

```text
100.64.0.0/10
```

Allocation rules:

- Each device receives a stable `/32` IPv4 address.
- IPv6 support should be designed into the model but can land after MVP.
- Addresses remain reserved until the device is deleted.
- The allocator must prevent collisions across networks.

## Hostnames and MagicDNS

Each device can have a custom hostname:

```text
macbook.mesh    -> 100.64.0.8
nas.mesh        -> 100.64.0.20
devbox.mesh     -> 100.64.0.31
```

Rules:

- Hostnames are unique within a network.
- Hostnames use lowercase letters, numbers, and hyphens.
- The default suffix is `mesh`.
- Custom DNS suffixes can be added later.
- Agents should route only mesh DNS queries to the local resolver and preserve
  the host system resolver for normal domains.

## ACL Model

MVP can start with full mesh connectivity. The first ACL version should use a
small JSON policy format:

```json
{
  "action": "accept",
  "src": ["user:alice", "tag:server"],
  "dst": ["tag:db:5432", "host:nas:22"]
}
```

Supported selectors:

- `user:<name>`
- `group:<name>`
- `tag:<name>`
- `host:<hostname>`
- `cidr:<cidr>`
- `*:<port>`

The control plane should compile ACLs into device-visible peer and route rules.
Agents can enforce local packet filtering later if the control plane visibility
filter is not enough.

## NAT Traversal

Connection preference order:

1. Same-LAN direct endpoint.
2. Public UDP endpoint discovered through STUN-like probing.
3. IPv6 endpoint when available.
4. Relay fallback.

The initial MVP can skip advanced NAT traversal and support direct endpoints
first. UDP hole punching and relay fallback should land soon after.

## Subnet Routes

A device can advertise LAN routes:

```text
office-router:
  virtual_ip: 100.64.0.10
  advertised_routes:
    - 192.168.1.0/24
    - 10.0.0.0/16
```

Routes require control plane approval before distribution.

## Security Boundaries

- Backend private credentials stay on devices.
- Control plane stores public credentials only.
- Control plane APIs require TLS.
- Device registration requires auth keys, OAuth, QR code flow, or admin
  approval.
- Auth keys can be one-time use, expiring, scoped, and pre-tagged.
- Device revocation removes the device from all peer maps.
- Audit logs record security-relevant changes.
- Relay services only forward encrypted packets.

## Deployment

MVP deployment:

- Single control plane container.
- SQLite database volume.
- Single relay container.
- CLI and agent binaries installed manually.

Production deployment:

- PostgreSQL.
- Optional Redis.
- Multiple relay regions.
- Object storage for logs and artifacts if needed.
- Observability through OpenTelemetry-compatible metrics and traces.

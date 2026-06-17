# v0.1 Detailed Design

## Goal

v0.1 delivers the smallest runnable custom mesh:

- Linux and macOS agents can register with one control plane.
- The control plane allocates stable overlay IPv4 addresses.
- Agents receive peer maps over HTTP polling.
- The custom userspace backend applies local TUN and peer session
  configuration.
- Two devices can ping each other by overlay IP when their endpoints are
  directly reachable.

The first backend is owned by tf-scale. The backend interface remains
pluggable so WireGuard, EasyTier, or other implementations can be added later.

## Decisions

| Area | Decision |
| --- | --- |
| Target platforms | Linux and macOS |
| Control protocol | HTTP JSON with agent polling |
| Backend | Self-developed userspace backend through `tfscale-net` |
| Backend crate | `tfscale-custom` |
| Interface name | `tfscale0` by default |
| Overlay CIDR | `100.64.0.0/10` |
| Transport | UDP direct endpoints first |
| Packet security | Backend-owned authenticated encryption |
| DNS | Deferred to v0.2 |
| NAT traversal | Deferred to v0.3 |
| Relay | Deferred to v0.4 |

## Scope

### Control Plane

- Start a single-node service with SQLite.
- Create auth keys.
- Register devices.
- Allocate one stable `/32` IPv4 address per device.
- Store backend type and public credential.
- Store latest heartbeat metadata.
- Generate a full-mesh peer map.
- Expose basic admin and agent HTTP APIs.

### Agent

- Generate and persist local machine identity.
- Generate and persist custom backend credentials.
- Register with the control plane using an auth key.
- Persist returned device identity and assigned overlay IP.
- Poll heartbeat and network map endpoints.
- Call the selected `NetworkBackend` implementation.
- Provide `up`, `down`, and `status` commands.

### Custom Backend

- Define backend-neutral traits and data structures in `tfscale-net`.
- Implement the default backend in `tfscale-custom`.
- Open and configure a userspace TUN interface on Linux and macOS.
- Encode and decode tf-scale packets.
- Encrypt and authenticate packets between peers.
- Send and receive peer traffic over UDP direct endpoints.
- Keep custom packet/session details inside `tfscale-custom`.

### CLI

- Create auth keys.
- List devices.
- Delete devices.
- Show simple JSON or table output.

## Out of Scope

- MagicDNS and local DNS configuration.
- ACLs beyond full-mesh visibility.
- Relay fallback.
- UDP hole punching.
- Subnet routers.
- Windows support.
- Web admin console.
- WireGuard backend implementation.
- EasyTier backend implementation.

## HTTP API

The MVP uses REST-style HTTP JSON to keep early development and debugging
simple. gRPC streaming can replace polling after the first mesh works.

### Admin API

```text
POST   /v1/auth-keys
GET    /v1/devices
DELETE /v1/devices/{device_id}
```

### Agent API

```text
POST /v1/agent/register
POST /v1/agent/heartbeat
GET  /v1/agent/network-map
```

### Register Device

Request:

```json
{
  "auth_key": "tskey-example",
  "hostname": "macbook",
  "machine_key": "machine-public-key",
  "backend_type": "tfscale",
  "backend_public_credential": "tfscale-public-key",
  "os": "macos",
  "arch": "arm64",
  "client_version": "0.1.0"
}
```

Response:

```json
{
  "device_id": "dev_123",
  "node_key": "node-secret",
  "ipv4": "100.64.0.2",
  "network_id": "net_default",
  "poll_interval_seconds": 5
}
```

### Heartbeat

Request:

```json
{
  "device_id": "dev_123",
  "node_key": "node-secret",
  "endpoints": [
    {
      "type": "lan",
      "address": "192.168.1.20",
      "port": 51820,
      "protocol": "udp"
    }
  ],
  "backend_status": {
    "backend_type": "tfscale",
    "interface": "tfscale0",
    "healthy": true
  }
}
```

### Network Map

Response:

```json
{
  "network_map_version": 12,
  "self_device": {
    "device_id": "dev_123",
    "hostname": "macbook",
    "ipv4": "100.64.0.2",
    "backend_type": "tfscale"
  },
  "peers": [
    {
      "device_id": "dev_456",
      "hostname": "devbox",
      "ipv4": "100.64.0.3",
      "backend_type": "tfscale",
      "backend_public_credential": "peer-tfscale-public-key",
      "endpoints": [
        {
          "type": "lan",
          "address": "192.168.1.30",
          "port": 51820,
          "protocol": "udp"
        }
      ],
      "allowed_routes": ["100.64.0.3/32"]
    }
  ]
}
```

## Backend Interface

`tfscale-agent` should call only the backend-neutral interface:

```rust
#[async_trait]
pub trait NetworkBackend {
    fn backend_type(&self) -> BackendType;
    fn capabilities(&self) -> BackendCapabilities;

    async fn ensure_credentials(&self) -> Result<BackendCredential>;
    async fn apply_local_config(&self, config: LocalBackendConfig) -> Result<()>;
    async fn apply_peer_map(&self, peers: Vec<PeerConfig>) -> Result<()>;
    async fn status(&self) -> Result<BackendStatus>;
    async fn shutdown(&self) -> Result<()>;
}
```

`PeerConfig` remains backend-neutral:

```text
device_id
hostname
overlay_ip
public_credential
endpoints
allowed_routes
```

`tfscale-custom` translates this into custom session state, packet routes, peer
keys, and UDP endpoint targets internally.

## Custom Data Plane

v0.1 should keep the custom data plane intentionally small.

### Local Interface

- Linux: use a userspace TUN crate or `/dev/net/tun`.
- macOS: use a userspace TUN crate or utun device.
- Configure `tfscale0` with the assigned `/32` overlay IP.
- Add an overlay route for `100.64.0.0/10`.

### Packet Flow

```text
Application packet
  -> tfscale0 TUN
  -> route lookup by destination overlay IP
  -> peer session lookup
  -> encrypt + frame
  -> UDP direct endpoint
  -> decrypt + validate
  -> peer TUN
```

### Packet Frame

The first frame format can be compact and versioned:

```text
version
message_type
source_device_id
destination_device_id
nonce
ciphertext
auth_tag
```

### Crypto

The backend should use a proven Rust crypto crate rather than custom primitive
implementations. The detailed choice can be finalized during backend
implementation, but the protocol should require:

- Authenticated encryption.
- Unique nonces per peer session.
- Public credentials stored in the control plane.
- Private credentials stored only on the device.
- Clear room for future key rotation.

## Persistence

v0.1 needs these tables:

- `auth_keys`
- `devices`
- `ip_allocations`
- `network_backends`
- `endpoints`

`dns_records`, `routes`, `acl_rules`, and `audit_events` can exist in the
model but do not need complete product behavior in v0.1.

## Milestones

1. Workspace skeleton and runnable binaries.
2. SQLite schema and control plane startup.
3. Auth key creation through CLI.
4. Agent registration and IP allocation.
5. `tfscale-net` trait plus `tfscale-custom` backend skeleton.
6. TUN interface open/configure on Linux.
7. TUN interface open/configure on macOS.
8. Packet framing and authenticated encryption.
9. UDP peer session send/receive.
10. Peer map generation and polling.
11. Two directly reachable devices can ping each other by overlay IP.

## Acceptance Criteria

1. A developer can start the control plane locally.
2. A developer can create an auth key with `tfscalectl`.
3. A Linux agent can register and receive an overlay IP.
4. A macOS agent can register and receive an overlay IP.
5. Both agents can poll and apply a peer map.
6. Both agents configure `tfscale0` through `tfscale-custom`.
7. A Linux and macOS device can ping each other by overlay IP when endpoints are
   directly reachable.
8. Deleting a device removes it from subsequent peer maps.

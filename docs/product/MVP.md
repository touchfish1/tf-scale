# MVP Scope

## Goal

Deliver the smallest useful tf-scale network:

- Two or more devices can join the same network.
- Devices receive stable virtual IPs.
- Devices can communicate over the custom userspace backend.
- Operators can inspect and remove devices.

## In Scope

### Control Plane

- Single organization.
- Single network.
- Auth key generation.
- Device registration.
- Backend public credential storage.
- Virtual IPv4 allocation from `100.64.0.0/10`.
- Device list API.
- Device rename API.
- Device deletion and revocation.
- Basic peer map generation.

### Agent

- Generate custom backend credentials.
- Register using an auth key.
- Persist device identity.
- Configure the local backend interface.
- Configure assigned overlay IP.
- Receive and apply peer map updates.
- Basic heartbeat.
- Linux and macOS support first.

### CLI

- `tfscalectl auth-key create`
- `tfscalectl device list`
- `tfscalectl device rename`
- `tfscalectl device delete`
- `tfscale-agent up`
- `tfscale-agent down`
- `tfscale-agent status`

## Out of Scope

- Multi-organization support.
- Enterprise SSO.
- Windows client.
- Mobile clients.
- Complex ACLs.
- MagicDNS and local DNS resolver.
- Exit nodes.
- Subnet routers.
- Multi-region relays.
- Advanced NAT traversal.
- Web admin console.
- Billing or SaaS tenant management.

## Acceptance Criteria

1. Start the control plane locally or through Docker Compose.
2. Create an auth key.
3. Register two agents.
4. Confirm each device receives a unique overlay IP.
5. Confirm each device sees the other device in its peer map.
6. Confirm Linux and macOS agents configure `tfscale0`.
7. Ping one directly reachable device from the other by overlay IP.
8. Delete a device and confirm it disappears from peer maps.

## First Demo Script

```sh
tfscaled serve --db ./tf-scale.db
tfscalectl auth-key create
tfscale-agent up --login-key <key>
tfscalectl device list
ping 100.64.0.2
```

See [v0.1 Detailed Design](V0_1_DETAILED_DESIGN.md) for the first-stage
implementation plan.

# MVP Scope

## Goal

Deliver the smallest useful tf-scale network:

- Two or more devices can join the same network.
- Devices receive stable virtual IPs.
- Devices can communicate over the WireGuard backend.
- Devices can use custom hostnames.
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

- Generate WireGuard backend credentials.
- Register using an auth key.
- Persist device identity.
- Configure the local backend interface.
- Configure assigned overlay IP.
- Receive and apply peer map updates.
- Basic heartbeat.
- Linux and macOS support first.

### DNS

- Store one hostname per device.
- Resolve `hostname.mesh` to the device overlay IP.
- Provide a minimal local DNS resolver or DNS proxy.

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
6. Ping one device from the other by overlay IP.
7. Rename a device.
8. Resolve the renamed device through `hostname.mesh`.
9. Delete a device and confirm it disappears from peer maps.

## First Demo Script

```sh
tfscaled serve --db ./tf-scale.db
tfscalectl auth-key create
tfscale-agent up --login-key <key>
tfscalectl device list
ping 100.64.0.2
ping devbox.mesh
```

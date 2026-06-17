# v0.1 Task Breakdown

This document breaks the first milestone into assignable workstreams. Owners
can be filled in once the team shape is known.

## Milestone Goal

v0.1 is complete when one Linux agent and one macOS agent can register with the
same control plane, receive overlay IPs, apply custom userspace backend
configuration, poll peer maps over HTTP, and ping each other by overlay IP when
directly reachable.

## Workstreams

| Workstream | Owner | Status |
| --- | --- | --- |
| Workspace and shared foundations | TBD | Done |
| Control plane API and persistence | TBD | In progress |
| CLI admin flows | TBD | In progress |
| Agent lifecycle and local state | TBD | In progress |
| Network backend abstraction | TBD | In progress |
| Custom backend skeleton | TBD | In progress |
| Linux TUN adapter | TBD | Not started |
| macOS TUN adapter | TBD | Not started |
| Packet crypto and framing | TBD | Not started |
| UDP peer transport | TBD | Not started |
| Peer map and polling loop | TBD | In progress |
| End-to-end validation | TBD | In progress |

## 1. Workspace and Shared Foundations

Deliverables:

- Rust workspace with core crates:
  - `tfscale-core`
  - `tfscale-control`
  - `tfscale-agent`
  - `tfscalectl`
  - `tfscale-net`
  - `tfscale-custom`
- Shared ID, error, config, and API payload types.
- Basic logging with `tracing`.
- Local development commands documented.

Acceptance:

- `cargo check --workspace` passes.
- `tfscaled`, `tfscale-agent`, and `tfscalectl` can print help/version output.

Current status:

- Complete. The workspace builds and the three binaries expose initial command
  surfaces.

## 2. Control Plane API and Persistence

Deliverables:

- `tfscaled serve --db ./tf-scale.db`.
- SQLite schema and migrations.
- HTTP JSON APIs for auth keys, devices, registration, heartbeat, and network
  maps.

Tasks:

- Implement SQLite migrations for:
  - `auth_keys`
  - `devices`
  - `ip_allocations`
  - `network_backends`
  - `endpoints`
- Implement auth key creation and hash storage.
- Implement device registration with auth key validation.
- Allocate stable `/32` addresses from `100.64.0.0/10`.
- Store backend type and public credential.
- Store heartbeat endpoint and backend status data.
- Return full-mesh peer maps.

Current status:

- Auth key creation, device registration, idempotent re-registration,
  heartbeat storage, device listing, device deletion, and full-mesh network map
  generation are implemented.
- Remaining work: migrations should move from inline startup SQL to migration
  files before the schema grows.

## 3. CLI Admin Flows

Deliverables:

- `tfscalectl auth-key create`
- `tfscalectl device list`
- `tfscalectl device delete`

Current status:

- `auth-key create`, `device list`, and `device delete` call the HTTP API.
- `--json` output exists.
- Remaining work: clearer error formatting.

## 4. Agent Lifecycle and Local State

Deliverables:

- `tfscale-agent up --login-key <key>`
- `tfscale-agent down`
- `tfscale-agent status`
- Local persisted identity and backend credentials.

Tasks:

- Create local state directory layout.
- Generate and persist machine identity.
- Generate and persist custom backend credentials through `tfscale-net`.
- Register with the control plane.
- Persist returned device ID, node key, network ID, and overlay IP.
- Send heartbeat payloads.
- Poll network map endpoint.
- Call backend local config and peer map methods.

Current status:

- `tfscale-agent up` creates local state, registers with the control plane,
  persists device identity, sends an initial heartbeat, fetches a network map,
  and calls backend local/peer config methods.
- `tfscale-agent status` reads local state and backend status.
- `tfscale-agent down` calls backend shutdown.
- Remaining work: long-running polling loop and richer backend status.

## 5. Network Backend Abstraction

Deliverables:

- Backend-neutral trait and models in `tfscale-net`.

Tasks:

- Define `NetworkBackend`.
- Define backend-neutral capabilities, credentials, local config, peer config,
  endpoints, and status.
- Add a mock backend for agent tests.
- Ensure custom packet/session concepts do not leak into shared models.

Current status:

- Initial backend-neutral trait and shared models exist in `tfscale-net`.
- Remaining work: add mock backend and agent-side tests.

## 6. Custom Backend Skeleton

Deliverables:

- `tfscale-custom` implements `NetworkBackend`.
- No external WireGuard dependency is required.

Tasks:

- Generate public/private backend credentials.
- Apply local config without touching platform networking until TUN adapters
  land.
- Apply peer map into internal session state.
- Report backend status.

Current status:

- Skeleton crate exists and implements the backend trait.
- Remaining work: real credentials, stored private key material, session state,
  and platform adapters.

## 7. Linux TUN Adapter

Deliverables:

- `tfscale-custom` can open/configure `tfscale0` on Linux.

Tasks:

- Choose TUN integration crate or direct `/dev/net/tun` path.
- Create or reuse `tfscale0`.
- Assign overlay IP.
- Add overlay route.
- Read/write IP packets.
- Implement shutdown cleanup.

## 8. macOS TUN Adapter

Deliverables:

- `tfscale-custom` can open/configure `tfscale0` or an utun device on macOS.

Tasks:

- Choose TUN integration crate or direct utun path.
- Create or reuse the interface.
- Assign overlay IP.
- Add overlay route.
- Read/write IP packets.
- Implement shutdown cleanup.

## 9. Packet Crypto and Framing

Deliverables:

- Versioned tf-scale packet frame.
- Authenticated encryption between peers.

Tasks:

- Choose a proven Rust crypto crate.
- Define frame version, message type, source, destination, nonce, ciphertext,
  and tag.
- Implement encode/decode.
- Implement nonce management.
- Add tests for tamper rejection and round-trip decode.

## 10. UDP Peer Transport

Deliverables:

- Direct endpoint UDP transport between peers.

Tasks:

- Bind a local UDP socket.
- Send encrypted frames to peer endpoints from the network map.
- Receive and dispatch frames to the correct peer session.
- Add endpoint health/status reporting.

## 11. Peer Map and Polling Loop

Deliverables:

- Full-mesh network map from control plane.
- Agent polling loop with version-aware application.

Current status:

- Control plane network map generation is implemented.
- Agent fetches and applies a network map once during `up`.
- Remaining work: long-running polling loop and version-aware application.

## 12. End-to-End Validation

Deliverables:

- Manual and scripted validation notes for Linux + macOS.

Tasks:

- Document required host permissions for TUN and route setup.
- Write a local demo script for control plane and CLI.
- Write a two-host validation checklist.
- Capture common failure modes:
  - TUN permission errors
  - route conflicts
  - endpoint not reachable
  - packet authentication failure
- Add smoke tests for API registration and peer map generation.

Current status:

- Manual smoke validation exists for control plane, CLI, registration,
  heartbeat, and network map.
- Remaining work: commit repeatable scripts and host dependency documentation.

## Suggested Assignment Order

1. Finish workspace/control/CLI foundations.
2. Build agent lifecycle against `tfscale-custom`.
3. Implement Linux TUN before macOS because Linux is easier to validate in CI.
4. Add packet framing and crypto.
5. Add UDP peer transport.
6. Add macOS TUN.
7. Finish peer polling, cleanup behavior, and end-to-end validation.

## Dependency Map

```text
workspace foundations
  -> control plane API
  -> CLI admin flows

workspace foundations
  -> backend abstraction
  -> custom backend skeleton
  -> agent lifecycle
  -> Linux TUN adapter
  -> macOS TUN adapter

custom backend skeleton
  -> packet crypto and framing
  -> UDP peer transport

control plane API + agent lifecycle + custom backend
  -> peer map polling
  -> end-to-end validation
```

## Initial Owner Suggestions

For a small team:

- Backend/control owner: control plane API, SQLite, peer maps.
- Agent/platform owner: agent lifecycle, `tfscale-net`, `tfscale-custom`, TUN.
- Data-plane owner: packet framing, crypto, UDP transport.
- Product/tooling owner: CLI, docs, validation scripts.

For one developer:

1. Workspace foundations.
2. Control plane API and SQLite.
3. CLI auth key and device list.
4. Agent registration with custom backend skeleton.
5. `tfscale-net` and Linux TUN adapter.
6. Packet framing and crypto.
7. UDP peer transport.
8. macOS TUN adapter.
9. End-to-end validation.

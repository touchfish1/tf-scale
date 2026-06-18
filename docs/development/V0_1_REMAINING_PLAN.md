# v0.1 Remaining Implementation Plan

This plan tracks the remaining work needed to finish the v0.1 minimal
pluggable mesh. It assumes the current workspace already has the control plane,
CLI, agent lifecycle skeleton, backend abstraction, and custom backend skeleton.

## Completion Target

v0.1 is complete when one Linux agent and one macOS agent can:

1. Register with the same `tfscaled` control plane.
2. Receive stable `100.64.0.0/10` overlay IPs.
3. Configure a local TUN interface through `tfscale-custom`.
4. Poll and apply peer map updates.
5. Exchange encrypted UDP packets directly.
6. Ping each other by overlay IP.
7. Stop seeing a deleted peer after device removal.

## Current Baseline

Implemented:

- Workspace builds and tests pass with `cargo test --workspace`.
- `tfscaled` supports auth key creation, device registration, heartbeat storage,
  device list/delete, and full-mesh peer maps.
- `tfscalectl` supports auth key creation and device list/delete.
- `tfscale-agent up/down/status` exists.
- The agent persists machine/device state, keeps running during `up`, sends
  recurring heartbeats, and applies changed peer maps.
- `tfscale-net` exposes backend-neutral traits and test utilities.
- `tfscale-custom` implements the backend trait and persists versioned X25519
  identity material, local config, and backend-owned peer session state.
- `tfscale-custom` has Linux TUN setup, TUN read/write boundaries, packet
  framing, crypto helpers, nonce handling, and runtime peer crypto material.

Known gaps:

- No UDP transport or TUN packet loop.
- No macOS TUN implementation.
- Linux TUN still needs privileged host validation.
- No repeatable end-to-end demo script.

## Phase 1: Agent Polling and Runtime Loop

Status: implemented in the current development branch.

Goal: turn `tfscale-agent up` from a one-shot command into a foreground runtime
that keeps the control plane and backend synchronized.

Tasks:

- Add an agent runtime loop after successful registration.
- Send heartbeat on `poll_interval_seconds`.
- Fetch network maps on the same interval.
- Track the last applied `network_map_version`.
- Apply backend peer config only when the version changes.
- Report backend status from `NetworkBackend::status()` instead of hardcoded
  values.
- Handle transient HTTP failures with retries and clear logs.
- Keep `Ctrl+C` shutdown graceful with backend shutdown.

Acceptance:

- Running `tfscale-agent up --login-key <key>` keeps the process alive.
- Heartbeats continue without restarting the command.
- Peer map application is skipped when the version is unchanged.
- Tests cover version-aware application and retry-safe loop helpers.

## Phase 2: Backend Credential Persistence and Session State

Status: core requirements implemented in the current development branch. The
backend now persists versioned X25519 identity material, local config, and
backend-owned peer session state. Endpoint health, nonce state, and transport
metadata will be added with framing and UDP transport.

Detailed requirements and implementation steps are tracked in
[v0.1 Phase 2 Backend State Plan](V0_1_PHASE_2_BACKEND_STATE_PLAN.md).

Goal: make `tfscale-custom` own durable local backend identity and in-memory
peer session state.

Tasks:

- Extend backend setup so the agent passes a backend state path or config
  directory.
- Store private backend material locally, outside the control plane.
- Derive or expose only the public credential for registration.
- Keep applied local config in backend state.
- Keep peer sessions keyed by `DeviceId` and overlay IP.
- Replace skeleton status with real state-derived health information.

Acceptance:

- Re-running the agent reuses the same backend public credential.
- Private material is never sent in API payloads.
- Unit tests prove peer maps add, update, and remove session entries.

## Phase 3: Control Plane and CLI Polish

Status: core requirements implemented in the current development branch.

Goal: finish MVP management gaps before data-plane work depends on them.

Tasks:

- Move inline SQLite schema creation into migration files.
- Add a migration runner on `tfscaled serve`.
- Implement device rename API.
- Add `tfscalectl device rename <device-id> <hostname>`.
- Improve CLI error formatting for HTTP and validation errors.
- Validate hostname uniqueness and basic hostname syntax.

Acceptance:

- Fresh startup creates schema through migrations.
- Existing databases can be opened without data loss.
- Device rename is reflected in `device list` and peer maps.
- CLI failures show concise actionable messages.

## Phase 4: Platform TUN Adapters

Status: Linux skeleton implemented in the current development branch. Privileged
Linux validation, packet read/write loops, and macOS support remain.

Detailed requirements and implementation steps are tracked in
[v0.1 Phase 4 TUN Adapter Plan](V0_1_PHASE_4_TUN_PLAN.md).

Goal: configure `tfscale0` on Linux and a macOS-compatible utun interface.

Tasks:

- Choose the TUN crate or direct OS integration path.
- Add platform modules under `tfscale-custom`.
- Open/create the interface.
- Assign the device overlay IP.
- Add route coverage for the overlay CIDR.
- Read packets from TUN and write packets back to TUN.
- Implement shutdown cleanup where supported.
- Document required privileges and OS-specific setup.

Acceptance:

- Linux agent creates/configures `tfscale0` or the chosen interface.
- macOS agent creates/configures a usable utun interface.
- Local TUN read/write can be tested with synthetic packets.
- Permission failures produce clear diagnostics.

## Phase 5: Packet Framing and Crypto

Status: core packet framing, crypto helpers, nonce handling, and runtime peer
credential material are implemented. UDP and TUN packet-loop integration remain
for Phase 6.

Detailed requirements and implementation steps are tracked in
[v0.1 Phase 5 Packet Framing and Crypto Plan](V0_1_PHASE_5_PACKET_CRYPTO_PLAN.md).

Goal: define the custom backend packet format and encrypt peer traffic.

Tasks:

- Select a proven AEAD/key agreement crate.
- Define versioned frame fields:
  - version
  - message type
  - source device ID
  - destination device ID
  - nonce
  - ciphertext
  - authentication tag
- Implement encode/decode helpers.
- Add nonce management per peer session.
- Reject malformed, replayed, or tampered frames.

Acceptance:

- Frame round-trip tests pass.
- Tampered ciphertext/tag tests fail as expected.
- Nonce reuse is prevented by API shape or explicit checks.

## Phase 6: UDP Peer Transport

Status: endpoint publication, UDP socket binding, peer endpoint selection,
transport status fields, IPv4 destination parsing, real frame-ID crypto session
construction, encrypted loopback UDP packet tests, and backend-level packet
send/receive helpers are implemented. TUN packet-loop wiring and privileged
Linux validation remain.

Detailed requirements and implementation steps are tracked in
[v0.1 Phase 6 UDP Peer Transport Plan](V0_1_PHASE_6_UDP_TRANSPORT_PLAN.md).

Goal: carry encrypted backend frames between peers over direct UDP endpoints.

Tasks:

- Bind a local UDP socket for the backend.
- Publish LAN endpoint candidates in heartbeat payloads.
- Select peer endpoints from network maps.
- Send encrypted frames to the selected endpoint.
- Receive frames, validate destination, decrypt, and write packets to TUN.
- Track peer endpoint health in backend status.

Acceptance:

- Two agents on the same LAN exchange backend frames.
- Invalid source or destination frames are dropped.
- Backend status reports socket and peer reachability.

## Phase 7: End-to-End Validation

Goal: make v0.1 reproducible for developers and reviewers.

Tasks:

- Add a local smoke script for control plane, CLI, and one agent.
- Add a two-host Linux/macOS checklist.
- Document required permissions for TUN and routes.
- Document common failure modes:
  - missing TUN permissions
  - route conflicts
  - blocked UDP port
  - stale peer map
  - packet authentication failure
- Capture expected command output for the happy path.

Acceptance:

- A fresh developer can follow the docs to run the demo.
- The checklist proves two devices receive distinct overlay IPs.
- Deleting a device removes it from subsequent peer maps.
- Directly reachable devices can ping by overlay IP.

## Suggested Execution Order

1. Agent polling and runtime loop.
2. Backend credential persistence and session state.
3. Control migrations, rename, and CLI polish.
4. Linux TUN adapter.
5. Packet framing and crypto.
6. UDP transport on Linux.
7. macOS TUN adapter.
8. End-to-end validation and documentation.

This order keeps control and agent behavior testable before platform networking
arrives, then proves the data plane on one platform before adding the second.

## Next Immediate Task

Start UDP peer transport implementation from
[v0.1 Phase 6 UDP Peer Transport Plan](V0_1_PHASE_6_UDP_TRANSPORT_PLAN.md).

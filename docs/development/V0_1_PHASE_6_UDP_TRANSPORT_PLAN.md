# v0.1 Phase 6 UDP Peer Transport Plan

## Goal

Phase 6 connects the custom backend data plane: read IP packets from TUN,
encrypt them with the Phase 5 frame/crypto helpers, send them over UDP to peer
endpoints, receive encrypted frames, decrypt them, and write plaintext packets
back to TUN.

The first implementation targets directly reachable LAN peers. NAT traversal,
relay fallback, endpoint probing, and macOS validation stay out of scope.

## Current Inputs

- `tfscale-custom` can configure and hold a Linux TUN device.
- TUN read/write boundaries exist through `TunDevice`.
- Packet frame, AEAD crypto, nonce, and replay helpers exist.
- Control plane heartbeat already accepts endpoint payloads.
- Control plane network maps already include peer endpoints.

## Current Status

Implemented in the current development branch:

- `NetworkBackend::local_endpoints()` exposes backend-owned endpoints.
- `tfscale-agent` publishes backend endpoints in heartbeat payloads.
- `tfscale-custom` binds a UDP socket after local config is applied.
- The custom backend reports a LAN UDP endpoint for control-plane publication.
- Peer map application selects LAN UDP endpoints into runtime transport state.
- Backend status includes UDP bound state, local endpoint count, peer endpoint
  count, and packet counters.
- `packet.rs` parses IPv4 destinations for later TUN-to-peer routing.
- `transport.rs` can send and receive UDP frames on loopback in tests.

Still remaining:

- Build full `PeerCryptoSession` values from real local/peer frame IDs.
- Route TUN packets through crypto sessions to selected UDP endpoints.
- Decrypt received UDP frames and write plaintext packets back to TUN.
- Add transport task lifecycle and cancellation around the blocking TUN loop.

## Architecture

Keep transport ownership inside `tfscale-custom`:

```text
tfscale-agent
  -> NetworkBackend::apply_local_config()
  -> NetworkBackend::apply_peer_map()
  -> NetworkBackend::local_endpoints()
  -> heartbeat endpoints

tfscale-custom
  -> TunDevice
  -> TransportRuntime
     -> UDP socket
     -> peer endpoint table
     -> PeerCryptoSession
     -> TUN read/write
```

Add backend-private modules under `crates/tfscale-custom/src/`:

```text
transport.rs
packet.rs
```

Responsibilities:

- `transport.rs`: UDP socket lifecycle, peer endpoint selection, packet loop,
  counters, health, and shutdown signal.
- `packet.rs`: minimal IPv4 destination parsing and route lookup by overlay IP.

## Backend API Changes

Extend `tfscale-net::NetworkBackend` with:

```rust
async fn local_endpoints(&self) -> Result<Vec<Endpoint>>;
```

The default mock backend should return a configurable vector, empty by default.
`tfscale-agent` should call this during heartbeat and convert each endpoint into
`EndpointPayload`.

Rationale: endpoint publication is backend-specific. The custom backend owns
the UDP listen socket and knows whether it is ready.

## Local Config Flow

When `apply_local_config()` succeeds:

1. Configure TUN as Phase 4 already does.
2. Bind UDP socket to `0.0.0.0:<listen_port>`.
3. Store local overlay IP, listen port, and UDP socket status.
4. Start the transport runtime once both TUN and UDP socket are ready.
5. Expose a LAN endpoint candidate such as:
   - kind: `lan`
   - protocol: `udp`
   - address: discovered primary LAN IPv4 or `127.0.0.1` in tests
   - port: configured listen port

For v0.1, endpoint discovery can be conservative:

- Prefer an explicit config/env override if added later.
- Otherwise discover a non-loopback IPv4 by opening a UDP socket toward a
  documentation address without sending packets.
- Fall back to `127.0.0.1` for local smoke tests.

## Peer Map Flow

When `apply_peer_map()` receives peers:

1. Persist the peer list as today.
2. Decode each peer public credential.
3. Parse local and peer device IDs into 16-byte frame IDs.
4. Construct or replace `PeerCryptoSession` for each peer.
5. Choose a first UDP endpoint per peer:
   - `EndpointKind::Lan`
   - `TransportProtocol::Udp`
   - otherwise no active endpoint.
6. Rebuild lookup maps:
   - overlay IP -> device ID
   - device ID -> endpoint
   - frame source ID -> crypto session

If a peer has no usable endpoint, keep it in state but mark transport status as
`waiting_for_endpoint`.

## Packet Send Path

TUN-to-UDP loop:

1. Read an IP packet from `TunDevice::read_packet()`.
2. Parse IPv4 destination address.
3. Find peer by destination overlay IP.
4. Encrypt the packet with that peer's `PeerCryptoSession::seal()`.
5. Send the frame to the selected UDP endpoint.
6. Increment packet/byte counters and update last-send timestamp.

Drop and count packets when:

- the packet is not IPv4.
- the destination is unknown.
- the peer has no UDP endpoint.
- encryption fails.
- UDP send fails.

Do not log plaintext packet bytes.

## Packet Receive Path

UDP-to-TUN loop:

1. Receive a UDP datagram.
2. Decode the frame header to identify source and destination.
3. Drop frames whose destination is not the local device frame ID.
4. Look up the peer crypto session by source frame ID.
5. Decrypt and replay-check with `PeerCryptoSession::open()`.
6. Validate plaintext destination or source against expected peer/local overlay
   addresses where practical.
7. Write plaintext bytes to `TunDevice::write_packet()`.
8. Increment packet/byte counters and update last-receive timestamp.

Drop and count frames when:

- frame parsing fails.
- source session is missing.
- destination is wrong.
- authentication or replay check fails.
- TUN write fails.

## Runtime and Concurrency

Use Tokio for the UDP side and keep the first TUN loop conservative:

- UDP socket: `tokio::net::UdpSocket`.
- Runtime task: one transport supervisor task with a cancellation channel.
- Shared peer/session table: `Arc<Mutex<...>>` first, replace with async-aware
  structures only if contention appears.
- TUN reads are blocking today. Run the TUN read loop with
  `tokio::task::spawn_blocking()` or move to an async TUN device in a follow-up.

Shutdown should:

1. Signal transport tasks to stop.
2. Stop accepting new packets.
3. Drop/close the UDP socket.
4. Join runtime tasks with a short timeout.
5. Run TUN cleanup as Phase 4 already does.

## Status Model

Backend status message should add transport fields:

- `udp_bound=true|false`
- `local_endpoints=N`
- `transport_peers=N`
- `reachable_peers=N`
- `tx_packets=N`
- `rx_packets=N`
- `tx_drops=N`
- `rx_drops=N`

`healthy` should be true when:

- TUN is configured and IO-ready.
- UDP socket is bound.
- Transport task is running.

It may remain healthy with zero peers, because an empty network is valid.

## Testing Strategy

Unit tests:

- IPv4 destination parsing.
- Unknown or non-IPv4 packets are dropped.
- Endpoint selection prefers LAN UDP.
- `local_endpoints()` returns the bound UDP endpoint.
- Peer map with no endpoint marks peer as waiting.
- Backend heartbeat conversion sends backend endpoints.

Integration-style tests without TUN privileges:

- Build two in-memory transport runtimes using UDP sockets bound to
  `127.0.0.1:0`.
- Use synthetic IP packet bytes instead of real TUN.
- Encrypt/send from A to B and decrypt/write into a mock packet sink.
- Reject wrong-destination frames.
- Reject replayed frames.
- Confirm counters update.

Manual validation:

- Start two Linux agents on the same LAN.
- Confirm each heartbeat publishes one LAN UDP endpoint.
- Confirm network maps include the peer endpoint.
- Ping peer overlay IP.
- Confirm backend status reports nonzero tx/rx counters.

## Implementation Steps

1. Add `NetworkBackend::local_endpoints()` and update mock/custom backends.
2. Update agent heartbeat to publish backend endpoints.
3. Add `packet.rs` with IPv4 destination parsing tests.
4. Add `transport.rs` UDP socket binding and local endpoint reporting.
5. Build peer endpoint selection and transport status structs.
6. Add synthetic UDP transport tests using loopback sockets.
7. Construct `PeerCryptoSession` values from local identity and peer map.
8. Connect TUN read/write loops behind runtime start/stop.
9. Update backend status and shutdown cleanup.
10. Add Linux manual validation notes for UDP traffic.

## Acceptance Criteria

- `cargo test --workspace` passes without network privileges beyond loopback UDP.
- Agent heartbeat publishes backend UDP endpoints.
- Control plane network maps deliver peer UDP endpoints.
- Synthetic two-peer transport test passes on loopback.
- Invalid, replayed, or wrong-destination frames are dropped.
- Backend status reports UDP socket, peer, and packet counters.
- Linux manual run can ping between two directly reachable agents by overlay IP.

## Follow-Up

Phase 7 will turn the manual path into repeatable smoke scripts and two-host
validation docs. Later versions can add endpoint ranking, NAT traversal, relay
fallback, async TUN, and public endpoint discovery.

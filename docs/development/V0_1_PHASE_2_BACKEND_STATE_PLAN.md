# v0.1 Phase 2 Backend State Plan

## Goal

Phase 2 makes `tfscale-custom` the owner of durable backend identity and peer
session state. The agent should only call `NetworkBackend`; custom private
material, peer indexes, and future packet/session internals stay inside
`tfscale-custom`.

## Current Baseline

Already implemented:

- `tfscale-agent up` runs continuously and applies changed peer maps.
- The agent passes its state directory to `CustomBackend`.
- `tfscale-custom` persists a JSON state file named `custom-backend.json`.
- The state file stores versioned X25519 identity material, local config, and
  backend-owned peer sessions.
- Public credentials use the `tfpk1_` typed prefix.
- Peer maps are validated and translated into stored session entries.
- Runtime indexes are built by device ID and overlay IP.
- State writes use a temporary file plus rename, and Unix builds set owner-only
  file permissions.
- Unit tests verify credential reuse, derived public keys, state version
  rejection, peer replacement, and credential format validation.

Still missing:

- Session metadata needed by framing, crypto, and UDP transport.
- Full endpoint health, nonce state, and transport status.

## Requirements

### Backend Identity

- Generate a durable private key locally on first use.
- Store the private key only in the backend state file.
- Derive the public credential from the private key.
- Return only the public credential from `ensure_credentials()`.
- Never include private material in control-plane API payloads or logs.
- Reuse the same public credential after agent restart.

Recommended first choice: use `x25519-dalek` for static backend identity keys.
If Phase 5 chooses a different crypto stack, this key type can still be wrapped
behind a `CustomIdentity` type before packet framing lands.

Public credentials should use a typed, versioned prefix:

```text
tfpk1_<base64url-x25519-public-key>
```

Private keys should be stored as raw base64url data inside backend state for
v0.1. Encrypting state at rest is deferred until a later hardening phase.

### State File

Use a versioned JSON structure while the project is pre-release:

```json
{
  "version": 1,
  "identity": {
    "key_id": "kid_...",
    "scheme": "x25519",
    "private_key": "base64url...",
    "public_key": "tfpk1_base64url...",
    "created_at": "2026-06-18T00:00:00Z",
    "rotated_from": null
  },
  "local_config": {
    "interface_name": "tfscale0",
    "overlay_ip": "100.64.0.2",
    "listen_port": 51820
  },
  "peers": []
}
```

Rules:

- Create parent directories before writing.
- Write state atomically where practical: write temporary file, then rename.
- Treat unsupported versions as errors.
- Preserve identity across peer map updates.
- Do not require TUN or UDP to exist before state can be created.
- Leave room for future identity rotation by storing `key_id`, `scheme`,
  `created_at`, and `rotated_from`.

### Peer Sessions

Peer map application should translate `Vec<PeerConfig>` into backend-owned
session entries:

- `device_id`
- `hostname`
- `overlay_ip`
- `public_key`
- `endpoints`
- `allowed_routes`
- `last_updated_at`
- future fields for endpoint health, nonce state, and transport state
- peer credential key ID once rotation is implemented

Maintain indexes in memory:

- by `DeviceId`
- by overlay `Ipv4Addr`

Behavior:

- Add new peers from the latest peer map.
- Update changed peers in place.
- Remove peers missing from the latest peer map.
- Reject peers with unsupported credential formats.
- Keep packet/framing details out of `tfscale-net`.

### Backend Status

`status()` should report state-derived health:

- `healthy=false` until TUN and UDP transport exist.
- Message should include whether identity exists, local config exists, and peer
  count.
- Later phases can add endpoint health and packet counters without changing the
  `NetworkBackend` trait.

## Implementation Steps

Completed:

1. Add crypto and encoding dependencies to `tfscale-custom`.
2. Replace placeholder `tfsk_...` / `tfpk_...` credentials with generated
   keypair material.
3. Introduce explicit state structs:
   - `CustomBackendState`
   - `CustomIdentity`
   - `StoredPeerSession`
4. Add version validation for loaded state.
5. Add atomic state writes.
6. Build in-memory peer indexes after `apply_peer_map()`.
7. Improve `status()` output from the new state model.
8. Add tests for:
   - public key is derived from private key
   - state survives reload
   - unsupported state version fails
   - peer add/update/remove behavior
   - private key is not returned by `ensure_credentials()`

Remaining:

1. Add endpoint health fields once UDP transport exists.
2. Add nonce/session crypto fields during packet framing.
3. Decide whether peer credentials should expose parsed key bytes to the framing
   module or remain encoded until transport setup.

## Key Rotation Design

Key rotation should be designed now but not implemented in v0.1 behavior.

State model requirements:

- Each local identity has a `key_id`.
- The active identity is the only key returned by `ensure_credentials()`.
- `rotated_from` can point to the previous local `key_id`.
- Future state versions may keep a short list of retired local identities for
  graceful peer migration.

Protocol requirements for later phases:

- Public credentials remain typed and versioned, starting with `tfpk1_`.
- Peer sessions should treat credential changes as session replacement.
- The control plane can continue storing an opaque public credential string in
  v0.1; no schema change is required yet.

Out of scope for v0.1:

- Rotation commands.
- Multi-key peer acceptance windows.
- Automatic rotation schedules.
- Control-plane rotation audit records.

Post-v0.1 direction:

- Implement manual operator-driven rotation first.
- Add automatic scheduled rotation only after peer migration and observability
  are reliable.
- Prefer an explicit CLI/admin action such as `tfscalectl device rotate-key`
  before background rotation policies.

## Acceptance Criteria

- `cargo test --workspace` passes.
- Re-running `ensure_credentials()` from a new backend instance returns the same
  public credential.
- The state file contains private identity material and a derived public key.
- `ensure_credentials()` returns only public material.
- Applying a second peer map removes stale peers.
- Backend status reflects identity, local config, and peer count.

## Out of Scope

- TUN interface creation.
- Packet frame encode/decode.
- AEAD encryption of packets.
- UDP sockets and endpoint probing.
- Key rotation.
- State encryption at rest.

## Storage Protection Direction

For v0.1, rely on OS profile permissions and create backend state files with
owner-only permissions where the platform supports it.

After MVP, prefer platform-native secret storage before custom state
encryption:

- macOS: Keychain for private identity material.
- Linux: Secret Service/libsecret when available, with owner-only file storage
  as the server-friendly fallback.
- Windows later: DPAPI or Windows Credential Manager.

Custom encrypted local state can be revisited for portable deployments, but it
should not block v0.1 or the first production hardening pass.

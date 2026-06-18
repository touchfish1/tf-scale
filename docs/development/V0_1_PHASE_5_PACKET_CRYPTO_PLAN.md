# v0.1 Phase 5 Packet Framing and Crypto Plan

## Goal

Phase 5 defines the encrypted packet format used by `tfscale-custom` and adds
testable encode/decode helpers. It does not bind UDP sockets or run a packet
loop; Phase 6 will connect these helpers to transport.

## Current Inputs

- Local backend identity is an X25519 static keypair stored by `tfscale-custom`.
- Peer credentials use `tfpk1_<base64url-x25519-public-key>`.
- Peer sessions are stored by device ID and overlay IP.
- TUN read/write boundaries exist but no packet loop consumes them yet.

## Current Status

Implemented in the current development branch:

- Binary frame header encode/decode with version, message type, source,
  destination, nonce, and ciphertext.
- XChaCha20Poly1305 seal/open helpers with header bytes as associated data.
- X25519 + HKDF-SHA256 peer session key derivation.
- Send nonce state and 64-packet replay window.
- Peer public credentials are decoded into backend runtime crypto material when
  peer maps are applied.
- Unit tests cover frame validation, round-trip encryption, tamper rejection,
  replay rejection, and unrelated session rejection.

Still remaining:

- Wire frame helpers into a TUN packet loop.
- Bind UDP sockets and route encrypted frames to peer endpoints.
- Use real local/peer device IDs from the transport runtime when creating
  `PeerCryptoSession` values.

## Dependency Choices

Use well-reviewed RustCrypto crates:

- `chacha20poly1305` for `XChaCha20Poly1305` AEAD.
- `hkdf` plus `sha2` for deterministic session key derivation.
- Existing `x25519-dalek` for shared secret agreement.

Rationale: XChaCha20 gives a 192-bit nonce, which is easier to use safely with
per-session counters and random salt material than a 96-bit nonce.

## Frame Format

All UDP payloads produced by the custom backend should use one binary frame:

```text
0      1      2      3      19     35     59       n
+------+------+------+------+------+------+--------+
| ver  | type | flags| rsv  | src  | dst  | nonce  | ciphertext...
+------+------+------+------+------+------+--------+
```

Fields:

- `version`: `1`.
- `type`: `1` for data packets. Reserve `2` for future key rotation control.
- `flags`: reserved for future use, must be `0` in v0.1.
- `reserved`: reserved byte, must be `0`.
- `src`: 16-byte `DeviceId` UUID bytes.
- `dst`: 16-byte `DeviceId` UUID bytes.
- `nonce`: 24-byte XChaCha20Poly1305 nonce.
- `ciphertext`: encrypted IP packet plus authentication tag.

Associated data must cover every header byte before `ciphertext`. This prevents
source, destination, version, or message type rewriting.

## Session Key Derivation

For each peer session:

1. Decode the peer `tfpk1_` public credential into a 32-byte X25519 public key.
2. Compute `shared_secret = local_private.diffie_hellman(peer_public)`.
3. Derive a 32-byte AEAD key with HKDF-SHA256:
   - salt: `b"tf-scale/custom/v1/session"`
   - info: sorted pair of local and peer device UUID bytes, followed by sorted
     pair of public credentials.
4. Keep derived keys in memory only. Persisted static private keys remain the
   root material for now.

Sorting the identity inputs gives both peers the same key regardless of sender.
Direction separation is handled by nonce prefixes.

## Nonce Strategy

Use a 24-byte nonce:

- bytes `0..8`: direction prefix.
- bytes `8..16`: random per-session salt generated when the session is created.
- bytes `16..24`: big-endian send counter.

Direction prefix is derived with HKDF from the same shared secret and ordered
sender/receiver device IDs, so each direction has a distinct prefix. The send
counter starts at `0` for each runtime session and increments after each
successful seal. Counter exhaustion must return an error.

For received frames, track the highest accepted counter per peer and a small
sliding bitmap window, initially 64 packets. Reject replays and packets too far
behind the window.

## Module Layout

Add backend-private modules under `crates/tfscale-custom/src/`:

```text
crypto.rs
frame.rs
nonce.rs
```

Responsibilities:

- `frame.rs`: binary header constants, encode/decode, validation errors.
- `crypto.rs`: peer public credential decode, X25519 agreement, HKDF key
  derivation, seal/open helpers.
- `nonce.rs`: send counter and receive replay window.

Keep these modules private to `tfscale-custom`; `tfscale-net` should remain
backend-neutral.

## Runtime Integration

Extend `StoredPeerSession` or runtime-only peer state with:

- decoded peer public key.
- derived AEAD session key.
- send nonce state.
- receive replay window.

`apply_peer_map()` should rebuild runtime crypto sessions when peers change.
Persisted peer data should stay simple and control-plane-shaped.

The first implementation decodes and validates peer public credentials into
runtime crypto material. Full `PeerCryptoSession` construction is deferred until
Phase 6, where the transport runtime owns local/peer frame IDs and packet
direction.

## Error Handling

Return `BackendError::CommandFailed` with actionable messages for:

- unsupported frame version or message type.
- malformed frame length.
- invalid peer credential.
- missing session for source or destination.
- authentication failure.
- replayed or stale nonce.
- send counter exhausted.

Avoid logging plaintext packet bytes.

## Testing Strategy

Unit tests:

- Frame header round trip.
- Reject short frames and unknown versions.
- Both peers derive identical session keys.
- Different peer pairs derive different keys.
- Seal/open round trip for an IPv4 packet payload.
- Tampered header or ciphertext fails to open.
- Replay window rejects duplicate nonces.
- Send counter increments and refuses overflow.

Integration-style tests:

- Build two in-memory `CustomBackend` instances with generated identities.
- Apply reciprocal peer sessions.
- Encrypt a synthetic IP packet from A to B.
- Decrypt on B and verify the original packet bytes.

## Acceptance Criteria

- `cargo test --workspace` passes without TUN or network privileges.
- Packet frame helpers are deterministic and covered by tests.
- AEAD authentication rejects tampered frames.
- Nonce reuse and basic replay are prevented in API-level tests.
- No UDP transport or packet loop code is required in this phase.

## Follow-Up

Phase 6 will bind UDP sockets, publish endpoints, send encrypted frames to peer
endpoints, receive frames, decrypt them, and write plaintext packets back to
TUN.

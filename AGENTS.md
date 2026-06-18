# Repository Guidelines

## Workspace & Entrypoints

Rust 2024 workspace with 6 crates under `crates/`. Each user-facing binary entrypoint:

| Package | Binary | Entrypoint |
|---|---|---|
| `tfscale-control` | `tfscaled` | `crates/tfscale-control/src/main.rs` |
| `tfscale-agent` | `tfscale-agent` | `crates/tfscale-agent/src/main.rs` |
| `tfscalectl` | `tfscalectl` | `crates/tfscalectl/src/main.rs` |
| `tfscale-core` | lib | shared IDs (`ids.rs`), errors (`error.rs`), protocol types (`protocol.rs`) |
| `tfscale-net` | lib | `NetworkBackend` trait, `BackendType`, `MockBackend` (behind `test-utils` feature) |
| `tfscale-custom` | lib | `CustomBackend` (X25519), TUN config, platform dispatch |

No CI/CD config exists yet. No `rustfmt.toml` or `clippy.toml` — uses Rust 2024 defaults.

## Prerequisites

- **Linux TUN**: run `scripts/linux-tun-check.sh` to verify `/dev/net/tun`, `ip` command, root/CAP_NET_ADMIN.
- **SQLite** via `sqlx` (runtime queries — no compile-time check, no `DATABASE_URL` needed).

## Commands

```bash
cargo check --workspace          # type-check all crates
cargo build --workspace          # build everything
cargo test --workspace           # all unit tests
cargo fmt --all                  # format (rustfmt defaults)
cargo clippy --workspace --all-targets  # lint
```

**Running the stack locally:**

```bash
# 1. Start control plane
cargo run -p tfscale-control -- serve --listen 127.0.0.1:8080

# 2. Create auth key
cargo run -p tfscalectl -- auth-key create

# 3. Agent with TUN (needs root / CAP_NET_ADMIN)
sudo TFSCALE_STATE_DIR=./state target/debug/tfscale-agent up --login-key "$KEY" --control-url http://127.0.0.1:8080
```

## Architecture Highlights

- **Overlay CIDR**: `100.64.0.0/10`. IP allocation starts at `100.64.0.2`, up to `100.64.0.254`.
- **Auth keys**: SHA-256 hashed, prefixed `tfk_`. CLI passes them with `--login-key` or `--admin-token`.
- **State files** (runtime artifacts, do not commit): `state.json` (agent), `custom-backend.json` (backend credentials/peers). Path controlled by `TFSCALE_STATE_DIR` env (defaults to `~/.local/share/tfscale` on Linux).
- **Shared protocol types**: All API request/response structs live in `tfscale-core/src/protocol.rs`. Never duplicate them in service crates.
- **Backend abstraction**: `tfscale-net::NetworkBackend` trait. `CustomBackend` in `tfscale-custom` is the sole implementation. Platform-specific TUN code at `crates/tfscale-custom/src/platform/` dispatches via `cfg(target_os = "linux")`.
- **All binaries use `clap` with `derive` feature** and subcommands.

## Testing

- Unit tests live beside code under `#[cfg(test)] mod tests`.
- `MockBackend` available via `tfscale-net` feature `test-utils` (used in `tfscale-agent` dev-deps).
- No integration tests exist yet. No snapshot testing.
- Control plane uses `sqlx::query()` (runtime, not compile-time checked).

## Commit Guidelines

Imperative present tense, no prefixes: `Start custom userspace backend MVP`. PRs should link relevant design docs and call out schema/protocol/network-behavior changes.

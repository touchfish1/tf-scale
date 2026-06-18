# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust workspace for `tf-scale`, a self-hosted mesh networking system. Workspace members live under `crates/`:

- `tfscale-core`: shared IDs, errors, and protocol models.
- `tfscale-net`: network backend abstraction.
- `tfscale-custom`: initial userspace backend implementation.
- `tfscale-control`: control plane service binary, published as `tfscaled`.
- `tfscale-agent`: node agent binary.
- `tfscalectl`: CLI client.

Design and planning material lives in `docs/`, with architecture notes under `docs/architecture/`, product scope under `docs/product/`, and development planning under `docs/development/`. The README mentions future directories such as `web/`, `proto/`, and `deploy/`; create them only when implementing those areas.

## Build, Test, and Development Commands

- `cargo check --workspace`: type-check all crates quickly.
- `cargo build --workspace`: compile every crate and binary.
- `cargo test --workspace`: run unit tests across the workspace.
- `cargo fmt --all`: format Rust code with rustfmt.
- `cargo clippy --workspace --all-targets`: run lint checks before submitting larger changes.
- `cargo run -p tfscale-control -- serve --listen 127.0.0.1:8080`: start the MVP control plane with a local SQLite database.

## Coding Style & Naming Conventions

Use Rust 2024 edition conventions and rustfmt defaults. Keep crate names lowercase with hyphens, module files lowercase with underscores when needed, and public types in `UpperCamelCase`. Prefer shared protocol and ID types from `tfscale-core` instead of duplicating request, response, or identifier structures in service crates. Keep backend-specific behavior behind `tfscale-net` traits and implementations such as `tfscale-custom`.

## Testing Guidelines

Place focused unit tests next to the code under `#[cfg(test)] mod tests`. Name tests by behavior, for example `rejects_invalid_auth_key` or `allocates_next_ipv4`. Run `cargo test --workspace` before opening a pull request. Add tests when changing protocol models, ID generation, IP allocation, backend abstractions, or API behavior.

## Commit & Pull Request Guidelines

Recent commits use concise imperative summaries such as `Start custom userspace backend MVP` and `Document pluggable network backend architecture`. Follow that style: start with a verb, keep the subject short, and avoid noisy prefixes unless the project later adopts them.

Pull requests should include a short description, the commands run for verification, linked issues or design docs when relevant, and screenshots only for future UI changes. Call out schema, protocol, or network-behavior changes explicitly.

## Security & Configuration Tips

Do not commit generated SQLite databases, auth keys, node keys, or local service logs. Keep secrets out of examples and prefer placeholder values such as `tfk_example`.

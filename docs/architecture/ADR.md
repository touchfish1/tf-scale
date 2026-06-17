# Architecture Decision Records

## ADR-001: Use a pluggable network backend with WireGuard first

Status: Accepted

Decision:

Define a backend boundary for encrypted node-to-node traffic. Use WireGuard as
the first MVP backend, but keep the control plane and product model independent
from WireGuard-specific configuration.

Rationale:

- Mature and widely deployed.
- Strong security model.
- Good Linux kernel support.
- Userspace options exist for macOS and Windows.
- Lets tf-scale focus on coordination, policy, DNS, routing, and product
  workflow instead of custom cryptography.
- Leaves room for EasyTier or a self-developed backend without replacing the
  control plane.

Constraints:

- WireGuard-specific fields and tooling must stay inside the WireGuard backend.
- Shared API and database models should use backend-neutral concepts such as
  nodes, peers, credentials, endpoints, routes, and capabilities.
- Backend implementations must explicitly declare capabilities so the control
  plane can avoid assuming that every backend behaves like WireGuard.

## ADR-002: Build a custom control plane

Status: Accepted

Decision:

Build a Headscale-like control plane owned by tf-scale rather than forking an
existing server.

Rationale:

- Product behavior can evolve without compatibility constraints.
- Data model can be designed for tf-scale from the start.
- ACLs, DNS, auth, route approval, and relay selection can use native concepts.

Tradeoff:

- More implementation work before the first complete release.
- Requires careful protocol and client design.

## ADR-003: Start with Rust

Status: Accepted

Decision:

Use Rust for the control plane, agent, relay, and CLI.

Rationale:

- Strong async networking through Tokio.
- Memory safety without a garbage collector.
- Good fit for long-running system daemons and networking infrastructure.
- Strong type system for shared protocol, policy, and configuration models.
- Good cross-platform binary distribution.
- One language across the core system reduces early project overhead.

Tradeoff:

- Early development is likely slower than Go.
- OS networking integration and backend wrapping require careful crate and
  platform choices.

## ADR-004: Use SQLite first, PostgreSQL later

Status: Accepted

Decision:

Use SQLite for the MVP and keep the persistence layer compatible with
PostgreSQL.

Rationale:

- SQLite keeps the first self-hosted deployment simple.
- The MVP does not need horizontal database scaling.
- PostgreSQL is the production target once multi-tenant and high-availability
  requirements appear.

## ADR-005: CLI before web console

Status: Accepted

Decision:

Build the CLI before the web admin console.

Rationale:

- The first milestone is network functionality, not dashboard polish.
- CLI flows are easier to test in integration environments.
- The web console can reuse stable admin APIs after the CLI validates them.

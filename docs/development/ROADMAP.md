# Development Roadmap

## v0.1: Minimal Pluggable Mesh

- Rust workspace and repository skeleton.
- Network backend abstraction.
- Custom userspace backend skeleton.
- Linux and macOS support.
- HTTP polling for agent registration, heartbeat, and network maps.
- Userspace TUN adapter design.
- Control plane service.
- SQLite persistence.
- Auth key creation.
- Device registration.
- Overlay IPv4 allocation.
- Agent identity persistence.
- Backend interface setup.
- Peer map generation.
- One Linux and one macOS device can ping each other by overlay IP when
  directly reachable.

## v0.2: Hostnames and MagicDNS

- Device rename.
- Hostname validation and uniqueness.
- Generated DNS records.
- Local DNS proxy.
- `hostname.mesh` resolution.
- Device deletion and revocation.

## v0.3-v0.4: Connectivity Probing and Relay Fallback

Detailed design:
[v0.2 Connectivity and Relay Plan](V0_2_CONNECTIVITY_RELAY_PLAN.md).
Development breakdown:
[v0.2 Connectivity and Relay Breakdown](V0_2_CONNECTIVITY_RELAY_BREAKDOWN.md).

- Endpoint discovery.
- LAN endpoint reporting.
- Public endpoint reporting.
- Basic STUN-like probing.
- Peer map endpoint ranking.
- Agent status diagnostics.
- Relay service.
- Agent-to-relay TLS connection.
- Encrypted packet relay.
- Control plane relay metadata.
- Relay health reporting.
- Direct-to-relay fallback behavior.

## Near Term: Tailscale-like Day-One Experience

Before the web console, prioritize the command-line and agent work needed for a
Tailscale-like day-one experience. Detailed plan:
[Tailscale-like Experience Plan](TAILSCALE_LIKE_EXPERIENCE_PLAN.md).

- System resolver integration for `ping hostname.mesh`.
- Agent service installation and restart persistence.
- Unified agent/control diagnostics.
- TLS and relay authentication hardening.
- Install scripts and release binaries.

## v0.5: Admin Console

- React plus Vite web application.
- Device list and detail pages.
- Rename and delete devices.
- Auth key management.
- Basic network status.

## v0.6: ACLs and Tags

- Device tags.
- User and group selectors.
- JSON ACL policy.
- ACL validation.
- Peer visibility filtering.
- Audit logs for policy changes.

## v0.7: Subnet Routing

- Route advertisement by agent.
- Route approval in control plane.
- Route distribution in peer maps.
- Agent system route configuration.
- Route status diagnostics.

## v0.8: Windows Support

- Windows agent packaging.
- Custom backend Windows integration.
- Route and DNS configuration.
- Service installation.

## Later: Alternative Backends

- WireGuard backend prototype.
- EasyTier backend prototype.
- Backend capability negotiation.
- Backend-specific diagnostics.
- Migration path for devices moving between WireGuard and alternative backends.

## v1.0: Production Readiness

- PostgreSQL support.
- Backup and restore guidance.
- Multi-organization support.
- Multi-region relay deployment.
- Upgrade-safe migrations.
- Observability.
- Security hardening.
- Release artifacts and installation docs.

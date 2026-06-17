# Development Roadmap

## v0.1: Minimal Pluggable Mesh

- Rust workspace and repository skeleton.
- Network backend abstraction.
- WireGuard backend implementation.
- Control plane service.
- SQLite persistence.
- Auth key creation.
- Device registration.
- Overlay IPv4 allocation.
- Agent identity persistence.
- Backend interface setup.
- Peer map generation.
- Two Linux or macOS devices can ping each other by overlay IP.

## v0.2: Hostnames and MagicDNS

- Device rename.
- Hostname validation and uniqueness.
- Generated DNS records.
- Local DNS proxy.
- `hostname.mesh` resolution.
- Device deletion and revocation.

## v0.3: Connectivity Probing

- Endpoint discovery.
- LAN endpoint reporting.
- Public endpoint reporting.
- Basic STUN-like probing.
- Peer map endpoint ranking.
- Agent status diagnostics.

## v0.4: Relay Fallback

- Relay service.
- Agent-to-relay TLS connection.
- Encrypted packet relay.
- Control plane relay metadata.
- Relay health reporting.
- Direct-to-relay fallback behavior.

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
- WireGuard userspace backend integration.
- Route and DNS configuration.
- Service installation.

## Later: Alternative Backends

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

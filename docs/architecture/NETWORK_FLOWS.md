# Network Flows

## Device Registration

```text
Agent                              Control Plane
  |                                      |
  | generate backend credentials         |
  |                                      |
  | register(auth_key, credential) ----> |
  |                                      | validate auth key
  |                                      | allocate overlay IP
  |                                      | reserve hostname
  | <---- device identity + config       |
  |                                      |
  | configure selected backend           |
  | start heartbeat                      |
```

Important rules:

- The private credential is generated locally and never sent to the control
  plane.
- The control plane stores only public credentials and device metadata.
- Registration should be idempotent after the device identity is persisted.

## Peer Map Update

```text
Agent A                         Control Plane                       Agent B
  |                                   |                                |
  | heartbeat + endpoints ----------> |                                |
  |                                   | <---------- heartbeat + endpoints
  |                                   | build network map               |
  | <---------- peer map update       |                                |
  |                                   | -------- peer map update -----> |
  | apply backend peer config         |                                |
  |                                   |        apply backend peer config
```

The peer map contains:

- Peer device ID.
- Peer backend public credential.
- Peer overlay IP.
- Candidate endpoints.
- Allowed routes.
- Relay fallback information.

## Direct Connection Attempt

```text
Agent A                     Control Plane / Probe                     Agent B
  |                                  |                                  |
  | UDP probe ---------------------> |                                  |
  | <------ observed public endpoint|                                  |
  | heartbeat + endpoints --------> | <-------- heartbeat + endpoints  |
  | <------------ peer map with endpoint candidates -----------------> |
  |                                                                     |
  |==== authenticated UDP probe to LAN/public endpoints ==============>|
  |<=== authenticated UDP probe response if NAT permits ===============|
  |                                                                     |
  | direct encrypted tunnel established                                 |
```

Connection preference:

1. LAN endpoint.
2. Public UDP endpoint.
3. IPv6 endpoint.
4. Relay fallback.

Endpoint discovery and relay fallback are detailed in
[v0.2 Connectivity and Relay Plan](../development/V0_2_CONNECTIVITY_RELAY_PLAN.md).

## Relay Fallback

```text
Agent A                  Relay Service                  Agent B
  |                            |                            |
  | TLS/WebSocket connection ->|<- TLS/WebSocket connection |
  | register device session -> | <- register device session |
  | encrypted backend frame -->|                            |
  |                            | --> encrypted backend frame |
  |                            |                            |
```

The relay forwards encrypted backend packets only. It cannot inspect user
traffic.

Agents continue direct probing while using relay. If a direct path becomes
healthy, packet routing switches back from relay to direct.

## Hostname Resolution

```text
Application
  |
  | query devbox.mesh
  v
Local DNS proxy on agent
  |
  | lookup local network map
  v
100.64.0.31
```

Rules:

- Mesh suffix queries are answered locally.
- Non-mesh queries continue to the system resolver.
- DNS records are derived from the control plane network map.

## Device Revocation

```text
Admin CLI / API                 Control Plane                 Remaining Agents
  |                                   |                              |
  | delete device ------------------> |                              |
  |                                   | mark device revoked           |
  |                                   | rebuild peer maps             |
  |                                   | ---- peer map updates ------> |
  |                                   |                              | remove revoked peer
```

Revoked devices lose access because other devices stop accepting them as peers.

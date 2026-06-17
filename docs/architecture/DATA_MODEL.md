# Data Model

This document describes the first stable domain model. Field names are
conceptual and can be adjusted during implementation.

## users

Represents a human operator.

```text
id
email
display_name
created_at
updated_at
```

MVP can skip full user management and use a bootstrap admin token.

## organizations

Represents a tenant or administrative boundary.

```text
id
name
created_at
updated_at
```

MVP uses one default organization.

## networks

Represents one overlay network.

```text
id
organization_id
name
ipv4_cidr
dns_suffix
created_at
updated_at
```

Default values:

```text
ipv4_cidr = 100.64.0.0/10
dns_suffix = mesh
```

## network_backends

Represents backend selection and backend-wide capability metadata for a
network.

```text
id
network_id
backend_type
capabilities
config
created_at
updated_at
```

Examples:

```text
backend_type = tfscale
capabilities = supports_userspace_tun, supports_static_peers

backend_type = easytier
capabilities = supports_relay, supports_nat_traversal, supports_dynamic_peers
```

Rules:

- `capabilities` and `config` can start as JSON for the MVP.
- The control plane can read capabilities, but should not interpret
  backend-private fields unless a product feature needs them.
- A future migration from the custom backend to WireGuard or EasyTier should
  create a new backend configuration and roll devices gradually instead of
  changing all device records in place.

## devices

Represents a node in the mesh.

```text
id
organization_id
network_id
user_id
hostname
machine_key
node_key
backend_type
backend_public_credential
ipv4
os
arch
client_version
last_seen_at
created_at
updated_at
deleted_at
```

Notes:

- `backend_type` identifies the selected network backend, such as `tfscale`
  for the MVP and `wireguard` or `easytier` for future implementations.
- `backend_public_credential` is safe to store in the control plane.
- Backend private credentials must never leave the device.
- `deleted_at` supports revocation audit history.

Backend-specific details such as custom session keys, WireGuard `allowed_ips`,
EasyTier network names, relay preferences, or implementation-specific endpoint
metadata should not be added directly to `devices`. Store them in backend
configuration records or derive them inside the backend implementation.

## auth_keys

Represents a key used to register devices.

```text
id
organization_id
network_id
key_hash
description
expires_at
used_at
reusable
preauthorized
tags
created_by
created_at
revoked_at
```

Security rules:

- Store hashes, not raw keys.
- Show raw keys only at creation time.
- Support one-time keys from the beginning.

## ip_allocations

Tracks overlay address ownership.

```text
id
network_id
device_id
ip
state
created_at
released_at
```

States:

- `reserved`
- `assigned`
- `released`

## dns_records

Represents generated and custom DNS records.

```text
id
network_id
device_id
name
type
value
created_at
updated_at
```

MVP only needs generated `A` records for device hostnames.

## endpoints

Stores latest device connectivity observations.

```text
id
device_id
type
address
port
protocol
source
latency_ms
last_seen_at
```

Types:

- `lan`
- `public`
- `ipv6`
- `relay`

## routes

Represents subnet routes advertised by devices.

```text
id
network_id
device_id
cidr
enabled
approved_by
created_at
updated_at
```

Subnet routes can wait until after MVP.

## acl_rules

Represents source-to-destination policy.

```text
id
organization_id
network_id
priority
action
src
dst
created_at
updated_at
```

ACLs can be stored as JSON in early versions, then normalized if policy
editing and search require it.

## audit_events

Records security and administrative changes.

```text
id
organization_id
actor_type
actor_id
action
target_type
target_id
metadata
created_at
```

Audit logging should start early, even if the first viewer is CLI-only.
